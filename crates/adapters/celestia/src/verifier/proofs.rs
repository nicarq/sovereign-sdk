use std::ops::Range;

// use borsh::{BorshDeserialize, BorshSerialize};
use celestia_types::nmt::{Namespace, NamespaceProof, NS_SIZE};
use celestia_types::row_namespace_data::{NamespacedShares, RowNamespaceData};
use serde::{Deserialize, Serialize};

use super::CelestiaSpec;
use crate::types::NamespaceData;
use crate::{CelestiaHeader, TxPosition};

// TODO: derive borsh Serialize, Deserialize <https://github.com/eigerco/celestia-node-rs/issues/155>
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct EtxProof {
    pub proof: Vec<EtxRangeProof>,
}

// TODO: derive borsh Serialize, Deserialize <https://github.com/eigerco/celestia-node-rs/issues/155>
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct EtxRangeProof {
    pub shares: Vec<Vec<u8>>,
    pub proof: NamespaceProof,
    pub start_share_idx: usize,
    pub start_offset: usize,
}

pub fn new_inclusion_proof(
    header: &CelestiaHeader,
    etx_rows: &NamespacedShares,
    rollup_data: &NamespaceData,
    blobs: &[<CelestiaSpec as sov_rollup_interface::da::DaSpec>::BlobTransaction],
) -> Vec<EtxProof> {
    let mut needed_tx_shares = Vec::new();

    // Extract (and clone) the position of each transaction
    for tx in blobs.iter() {
        let (_, position) = rollup_data
            .relevant_pfbs
            .get(tx.hash.0.as_slice())
            .expect("commitment must exist in map");
        needed_tx_shares.push(position.clone());
    }

    subnamespace_inclusion_proofs(header.square_size(), etx_rows, &mut needed_tx_shares)
}

/// The namespace is a contiguous set of shares from the EDS.
/// It's arranged in the rows corresponding to the EDS rows, with the first and last row included
/// being not necessarily the full width. E.g.:
///```ascii
/// [ A1, A2, B1, B2]
/// [ B3, B4, B5, B6]
/// [ B7, C1, C2, C3]
/// ```
///
/// Here, the NamespacedShares for namespace B will be shaped as [[B1, B2], [B3, B4, B5, B6], [B7]].
/// `NamespacedShares` also includes row proofs for every included (sub)row. These are merkle
/// proofs whose root is the EDS row root - for example, in case above, B3-B6 make a complete row
/// so the merkle proof is trivial, while the two partial rows B1-B2 and B7 respectively each get a
/// merkle proof that can be used to verify inclusion in the EDS row. (In turn, the extended data
/// header contains the proofs necessary to verify inclusion of each row root.)
///
/// This function proves the inclusion of specific shares within that namespace, as determined by
/// `ranges_to_prove`. It uses the known namespace shares outside of the desired range, alongside
/// the existing proof for the shares outside the namespace to generate a narrower merkle range
/// proof.
///
/// Because the proofs are based on row proofs and are rooted in the row roots, ranges that span
/// multiple rows get separate proofs for each fragment on a different row.
/// E.g. passing a B2-B3 range using the same example above would generate two proofs, one for
/// share B2 and one for share B3; or, hypothetically, passing a B2-B7 range would generate three
/// separate proofs: one for B2, one for the B3-B6 row, and one for B7. This is why `EtxProof` is
/// itself a vec of proofs.
///
/// NOTE that the ranges have to be non-overlapping and sorted. This is because the function
/// iterates on the row, generating sub-proofs for any range that overlaps with the row.
/// TODO: it may be possible to rewrite the function to iterate only over the ranges, fetching the
/// corresponding row(s) from the namespace data and generating proof(s) correspondingly, but
/// without ever iterating on the namespace rows. This will probably be cleaner and shorter than
/// this code and will relax the implicit ordering and non-overlap requirement.
fn subnamespace_inclusion_proofs(
    row_length: usize,
    namespace_with_proof: &NamespacedShares,
    ranges_to_prove: &mut [TxPosition], // When proving blobs, start_offset will always be 0, so these
                                        // can become just ranges instead of `TxPosition`s
) -> Vec<EtxProof> {
    let mut proofs: Vec<EtxProof> = Vec::with_capacity(ranges_to_prove.len());
    let mut ranges_to_prove = ranges_to_prove.iter_mut().peekable();
    let mut current_tx_proof: EtxProof = EtxProof { proof: Vec::new() };

    let Some(first_row) = namespace_with_proof.rows.first() else {
        // If the namespace is empty (no rows), there's nothing to prove
        return vec![];
    };
    let first_share = first_row
        .shares
        .first()
        .expect("Row must contain at least one share. This is a bug in either Lumina or the connected Celestia Node.");

    let namespace_id = Namespace::from_raw(&first_share[..NS_SIZE]).expect(
        "Invalid namespace id. This is a bug in either Lumina or the connected Celestia Node.",
    );

    // We iterate over the rows, and check if any range to proof overlaps the current row at least
    // partially. If so, we generate a sub-proof from the row proof for that overlap.
    // TODO: possibly refactor this as per the doc comment
    for (row_idx, namespace_row) in namespace_with_proof.rows.iter().enumerate() {
        // The first row's namespace shares may not start on column 0
        // e.g. [B1, B2] from the doc comment start on column 2
        let namespace_start_in_row = if row_idx == 0 {
            // NOTE that this assumes the namespace continues for more than one row, and thus the
            // first partial row must be at the very right of the row (to be contiguous with the
            // next row).
            // If the entire namespace is contained within one (partial) row, this will not be the
            // actual namespace start index in the row.
            // However, this value is used to skip rows when the next range to prove does not start
            // on the current row. If the entire namespace is contained within one row, the exact
            // value of the start index is unimportant.
            row_length - namespace_row.shares.len()
        } else {
            0
        };

        // Get the next range. If any part of it overlaps with the current row, generate a proof
        // for that part. If it spans more than one row, then we'll prove the part that's on the
        // current row, then mutate the range so the next iteration proves the part on the next
        // row, hence peek_mut().
        while let Some(next_needed_range) = ranges_to_prove.peek_mut() {
            let row_start_idx = row_length.checked_mul(row_idx).expect("invalid row");
            // "Relative" to the namespace row - which, again, may not start at the beginning of
            // the EDS row if it's the first row in the namespace. We don't correct for that here,
            // hence relative.
            let relative_start_column_number = next_needed_range
                .share_range
                .start
                .checked_sub(row_start_idx)
                .expect("invalid row");
            // If the next needed share doesn't fall in this row, skip it
            // As noted above, `namespace_start_in_row` isn't strictly correct if the namespace is
            // a single partial row, but this doesn't matter. E.g. consider:
            // `[A1, B1, B2, C1]` - the true namespace start index is 1, but will be calculated as
            // 2. This will cause the row to be skipped if 2 + relative_start > 4, i.e. if
            // relative_start_column > 2, but this will never be the case because the entire
            // namespace is two wide so the possible relative share indices are [0, 1].
            //
            // The calculation thus runs as if the EDS row shape had been `[A1, A2, B1, B2]`,
            // and that's fine. (In a multi-row namespace, it's possible that the next row starts
            // with `[B3, ...]` and that's what the skip check is for - in this case, a relative
            // start index of 2 or greater IS possible and will skip the row.
            if relative_start_column_number
                .checked_add(namespace_start_in_row)
                .expect("invalid row")
                > row_length
            {
                break;
            }
            // Same as relative_start_column_number - will not be the "real" column index on
            // an offset first row
            let relative_end_column_number = next_needed_range
                .share_range
                .end
                .checked_sub(row_start_idx)
                .expect("invalid row");

            // If the range ends on this row
            // As above, this isn't strictly real yet can't erroneously trigger on single-row
            // namespaces
            if relative_end_column_number
                .checked_add(namespace_start_in_row)
                .expect("invalid row")
                <= row_length
            {
                let range_shares = copy_raw_shares(
                    namespace_row,
                    relative_start_column_number..relative_end_column_number,
                );
                // supply all the know shares from the namespace row to each side of the range
                // we're proving
                let proof = namespace_row
                    .proof
                    .narrow_range(
                        &namespace_row.shares[0..relative_start_column_number],
                        &namespace_row.shares
                            [relative_end_column_number..namespace_row.shares.len()],
                        *namespace_id,
                    )
                    .unwrap();

                current_tx_proof.proof.push(EtxRangeProof {
                    shares: range_shares,
                    proof: proof.into(),
                    start_offset: next_needed_range.start_offset,
                    start_share_idx: next_needed_range.share_range.start,
                });
                // We've finished proving this range. Save the total proof and consume from the
                // iterator.
                proofs.push(current_tx_proof);
                let _ = ranges_to_prove.next();
                // Start a new blank collection of proofs for the next range (if any)
                current_tx_proof = EtxProof { proof: Vec::new() };
            } else {
                // Range to prove does NOT end on the same row.
                // We push a proof of the partial range on this row, then mutate the range in-place
                // to start from the next row, for which we will generate a proof on the next
                // iteration(s).
                let shares = copy_raw_shares(
                    namespace_row,
                    relative_start_column_number..relative_end_column_number,
                );
                let proof = namespace_row
                    .proof
                    .narrow_range(
                        &namespace_row.shares[0..relative_start_column_number],
                        &namespace_row.shares[relative_end_column_number..row_length],
                        *namespace_id,
                    )
                    .unwrap();

                current_tx_proof.proof.push(EtxRangeProof {
                    shares,
                    proof: proof.into(),
                    start_offset: next_needed_range.start_offset,
                    start_share_idx: next_needed_range.share_range.start,
                });

                // Set the range to start on the first share of the next row
                next_needed_range.share_range.start = row_idx
                    .checked_add(1)
                    .expect("invalid row id")
                    .checked_mul(row_length)
                    .expect("invalid square size");
                next_needed_range.start_offset = 0;

                break;
            }
        }
    }
    proofs
}

fn copy_raw_shares(row: &RowNamespaceData, range: Range<usize>) -> Vec<Vec<u8>> {
    row.shares[range].iter().map(|s| s.to_vec()).collect()
}
