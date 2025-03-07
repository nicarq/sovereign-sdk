use std::convert::Infallible;

use borsh::{BorshDeserialize, BorshSerialize};
use celestia_types::consts::appconsts::v3::SUBTREE_ROOT_THRESHOLD;
use celestia_types::nmt::{Namespace, NS_SIZE};
use celestia_types::row_namespace_data::NamespaceData;
use celestia_types::{Commitment, DataAvailabilityHeader};
use nmt_rs::NamespacedSha2Hasher;
use serde::{Deserialize, Serialize};
use sov_rollup_interface::da::{
    self, BlobReaderTrait, BlockHashTrait as BlockHash, DaSpec, RelevantBlobs, RelevantProofs,
};
use sov_rollup_interface::Buf;

pub mod address;
pub mod proofs;

use tracing::debug;

use self::address::CelestiaAddress;
use self::proofs::*;
use crate::shares::{NamespaceGroup, Share};
use crate::types::{BlobWithSender, ValidationError};
use crate::utils::read_varint;
use crate::{pfb_from_iter, CelestiaHeader};

#[derive(Clone)]
pub struct CelestiaVerifier {
    pub rollup_batch_namespace: Namespace,
    pub rollup_proof_namespace: Namespace,
}

pub const PFB_NAMESPACE: Namespace = Namespace::const_v0([0, 0, 0, 0, 0, 0, 0, 0, 0, 4]);
pub const PARITY_SHARES_NAMESPACE: Namespace = Namespace::PARITY_SHARE;

impl BlobReaderTrait for BlobWithSender {
    type Address = CelestiaAddress;
    type BlobHash = TmHash;

    fn sender(&self) -> CelestiaAddress {
        self.sender.clone()
    }

    fn hash(&self) -> Self::BlobHash {
        TmHash(tendermint::Hash::Sha256(self.hash.0))
    }

    fn verified_data(&self) -> &[u8] {
        self.blob.accumulator()
    }

    fn total_len(&self) -> usize {
        self.blob.total_len()
    }

    #[cfg(feature = "native")]
    fn advance(&mut self, num_bytes: usize) -> &[u8] {
        self.blob.advance(num_bytes);
        self.verified_data()
    }
}

#[derive(Debug, PartialEq, PartialOrd, Ord, Clone, Eq, Hash, Serialize, Deserialize)]
pub struct TmHash(pub tendermint::Hash);

impl BorshSerialize for TmHash {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> Result<(), std::io::Error> {
        BorshSerialize::serialize(self.inner(), writer)
    }
}

impl BorshDeserialize for TmHash {
    fn deserialize(buf: &mut &[u8]) -> Result<Self, std::io::Error> {
        let bytes = <[u8; 32] as BorshDeserialize>::deserialize(buf)?;
        Ok(Self(tendermint::Hash::Sha256(bytes)))
    }

    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let bytes = <[u8; 32]>::deserialize_reader(reader)?;
        Ok(Self(tendermint::Hash::Sha256(bytes)))
    }
}

impl AsRef<[u8]> for TmHash {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl core::fmt::Display for TmHash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "0x{}", self.0)
    }
}

impl core::str::FromStr for TmHash {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let stripped = s.strip_prefix("0x").unwrap_or(s);
        let inner = tendermint::Hash::from_str(stripped)?;
        Ok(TmHash(inner))
    }
}

impl TmHash {
    pub fn inner(&self) -> &[u8; 32] {
        match self.0 {
            tendermint::Hash::Sha256(ref h) => h,
            // Hack: when the hash is None, we return a hash of all 255s as a placeholder.
            // TODO: add special casing for the genesis block at a higher level
            tendermint::Hash::None => unreachable!("Only the genesis block has a None hash, and we use a placeholder in that corner case")
        }
    }
}

impl BlockHash for TmHash {}

impl From<TmHash> for [u8; 32] {
    fn from(val: TmHash) -> Self {
        *val.inner()
    }
}

impl TryFrom<[u8; 32]> for TmHash {
    type Error = Infallible;

    fn try_from(value: [u8; 32]) -> Result<Self, Self::Error> {
        Ok(Self(tendermint::Hash::Sha256(value)))
    }
}

#[derive(
    Default,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Eq,
    Clone,
    BorshDeserialize,
    BorshSerialize,
)]
pub struct CelestiaSpec;

impl DaSpec for CelestiaSpec {
    type SlotHash = TmHash;

    type BlockHeader = CelestiaHeader;

    type BlobTransaction = BlobWithSender;

    type TransactionId = TmHash;

    type Address = CelestiaAddress;

    type InclusionMultiProof = Vec<EtxProof>;

    type CompletenessProof = NamespaceData;

    type ChainParams = RollupParams;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RollupParams {
    pub rollup_batch_namespace: Namespace,
    pub rollup_proof_namespace: Namespace,
}

impl da::DaVerifier for CelestiaVerifier {
    type Spec = CelestiaSpec;

    type Error = ValidationError;

    fn new(params: <Self::Spec as DaSpec>::ChainParams) -> Self {
        Self {
            rollup_proof_namespace: params.rollup_proof_namespace,
            rollup_batch_namespace: params.rollup_batch_namespace,
        }
    }

    #[cfg_attr(feature = "bench", sov_modules_macros::cycle_tracker)]
    fn verify_relevant_tx_list(
        &self,
        block_header: &<Self::Spec as DaSpec>::BlockHeader,
        relevant_blobs: &RelevantBlobs<<Self::Spec as DaSpec>::BlobTransaction>,
        relevant_proofs: RelevantProofs<
            <Self::Spec as DaSpec>::InclusionMultiProof,
            <Self::Spec as DaSpec>::CompletenessProof,
        >,
    ) -> Result<(), Self::Error> {
        // Validate that the provided DAH is well-formed
        block_header.validate_dah()?;

        Self::verify_txs(
            block_header,
            &relevant_blobs.proof_blobs,
            self.rollup_proof_namespace,
            relevant_proofs.proof.inclusion_proof,
            relevant_proofs.proof.completeness_proof,
        )?;

        Self::verify_txs(
            block_header,
            &relevant_blobs.batch_blobs,
            self.rollup_batch_namespace,
            relevant_proofs.batch.inclusion_proof,
            relevant_proofs.batch.completeness_proof,
        )?;

        Ok(())
    }
}

impl CelestiaVerifier {
    fn verify_txs(
        block_header: &CelestiaHeader,
        txs: &[BlobWithSender],
        namespace: Namespace,
        inclusion_proof: Vec<EtxProof>,
        completeness_proof: NamespaceData,
    ) -> Result<(), ValidationError> {
        // Check the validity and completeness of the rollup row proofs, against the DAH.
        // Extract the data from the row proofs and build a namespace_group from it
        let verified_shares =
            Self::verify_row_proofs(namespace, completeness_proof, &block_header.dah)?;

        if verified_shares.is_empty() {
            if !txs.is_empty() {
                return Err(ValidationError::MissingTx);
            }
            Ok(())
        } else {
            Self::verify_inclusion_proof(
                namespace,
                block_header,
                verified_shares,
                txs,
                inclusion_proof,
            )
        }
    }

    fn verify_inclusion_proof(
        namespace: Namespace,
        block_header: &CelestiaHeader,
        verified_shares: NamespaceGroup,
        txs: &[BlobWithSender],
        inclusion_proof: Vec<EtxProof>,
    ) -> Result<(), ValidationError> {
        // Check the e-tx proofs...
        // TODO(@preston-evans98): Remove this logic if Celestia adds blob.sender metadata directly into blob
        let mut tx_iter = txs.iter();
        let mut tx_proofs = inclusion_proof.into_iter();
        let square_size = block_header.dah.row_roots().len();
        for blob in verified_shares.blobs() {
            if blob.is_padding() {
                debug!("Ignoring namespace padding blob. Sequence length 0.");
                continue;
            }
            // Get the etx proof for this blob
            let Some(tx_proof) = tx_proofs.next() else {
                return Err(ValidationError::InvalidEtxProof("not all blobs proven"));
            };

            // Force the row number to be monotonically increasing
            let start_offset = tx_proof.proof[0].start_offset;

            // Verify each sub-proof and flatten the shares back into a sequential array
            // First, enforce that the sub-proofs cover a contiguous range of shares
            for i in 1..tx_proof.proof.len() {
                let l = &tx_proof.proof[i.checked_sub(1).expect("invalid tx proof len")];
                let r = &tx_proof.proof[i];
                assert_eq!(
                    l.start_share_idx.saturating_add(l.shares.len()),
                    r.start_share_idx
                );
            }
            let mut tx_shares = Vec::new();
            // Then, verify the sub proofs
            for sub_proof in tx_proof.proof.into_iter() {
                let row_num = sub_proof
                    .start_share_idx
                    .checked_div(square_size)
                    .expect("the square size is invalid");
                let root = &block_header.dah.row_roots()[row_num];
                sub_proof
                    .proof
                    .verify_range(root, &sub_proof.shares, PFB_NAMESPACE.into())
                    .map_err(|_| ValidationError::InvalidEtxProof("invalid sub proof"))?;
                tx_shares.extend(
                    sub_proof
                        .shares
                        .into_iter()
                        .map(|share_vec| Share::new(share_vec.into())),
                );
            }

            // Next, ensure that the start_index is valid
            if !tx_shares[0].is_valid_tx_start(start_offset) {
                return Err(ValidationError::InvalidEtxProof("invalid start index"));
            }

            // Collect all of the shares data into a single array
            let trailing_shares = tx_shares[1..]
                .iter()
                .flat_map(|share| share.data_ref().iter());
            let tx_data: Vec<u8> = tx_shares[0].data_ref()[start_offset..]
                .iter()
                .chain(trailing_shares)
                .copied()
                .collect();

            // Deserialize the pfb transaction
            let (len, len_of_len) = {
                let cursor = std::io::Cursor::new(&tx_data);
                read_varint(cursor).expect("tx must be length prefixed")
            };
            let mut cursor =
                std::io::Cursor::new(&tx_data[len_of_len..len_of_len.saturating_add(len as usize)]);

            let pfb = pfb_from_iter(&mut cursor, len as usize)
                .map_err(|_| ValidationError::InvalidEtxProof("invalid pfb"))?;

            // Verify the sender and data of each blob which was sent into this namespace
            for (blob_idx, nid) in pfb.namespaces.iter().enumerate() {
                if nid != namespace.as_bytes() {
                    continue;
                }
                let tx: &BlobWithSender = tx_iter.next().ok_or(ValidationError::MissingTx)?;
                if tx.sender.to_string() != pfb.signer {
                    return Err(ValidationError::InvalidSigner);
                }

                let blob_ref = blob.clone();

                let mut blob_iter = blob_ref.data();
                let mut blob_data = vec![0; blob_iter.remaining()];
                blob_iter.copy_to_slice(blob_data.as_mut_slice());

                let tx_data = tx.verified_data();

                assert!(
                    tx_data.len() <= blob_data.len(),
                    "claimed data must not be larger smaller than blob data"
                );
                for (l, r) in tx_data.iter().zip(blob_data.iter()) {
                    assert_eq!(l, r, "claimed data must match observed data");
                }

                // Link blob commitment to e-tx commitment
                let expected_commitment = Commitment::from_shares(
                    namespace,
                    &blob_ref.celestia_shares(),
                    SUBTREE_ROOT_THRESHOLD,
                )
                .map_err(|_| ValidationError::InvalidEtxProof("failed to recreate commitment"))?;

                assert_eq!(
                    &pfb.share_commitments[blob_idx][..],
                    expected_commitment.hash()
                );
            }
        }

        if tx_proofs.next().is_some() {
            return Err(ValidationError::InvalidEtxProof("more proofs than blobs"));
        }

        Ok(())
    }

    fn verify_row_proofs(
        namespace: Namespace,
        row_proofs: NamespaceData,
        dah: &DataAvailabilityHeader,
    ) -> Result<NamespaceGroup, ValidationError> {
        let mut row_proofs = row_proofs.rows.into_iter();
        // Check the validity and completeness of the rollup share proofs
        let mut verified_shares = Vec::new();
        for row_root in dah.row_roots().iter() {
            // TODO: short circuit this loop at the first row after the rollup namespace
            if row_root.contains::<NamespacedSha2Hasher<NS_SIZE>>(namespace.into()) {
                let row_proof = row_proofs.next().ok_or(ValidationError::InvalidRowProof)?;
                row_proof
                    .proof
                    .verify_complete_namespace(row_root, &row_proof.shares, namespace.into())
                    .expect("Proofs must be valid");

                for leaf in row_proof.shares {
                    verified_shares.push(leaf);
                }
            }
        }
        Ok(NamespaceGroup::from_shares(verified_shares))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn test_serialize_roundtrip(raw: [u8; 32]) {
        let tm_hash = TmHash::try_from(raw).unwrap();
        let serde_serialized = serde_json::to_string(&tm_hash).unwrap();
        let serde_deserialized: TmHash = serde_json::from_str(&serde_serialized).unwrap();

        assert_eq!(tm_hash, serde_deserialized);

        let borsh_serialized = borsh::to_vec(&tm_hash).unwrap();
        let borsh_deserialized: TmHash = borsh::from_slice(&borsh_serialized).unwrap();

        assert_eq!(tm_hash, borsh_deserialized);
    }

    fn test_str_roundtrip(raw: [u8; 32]) {
        let tm_hash = TmHash::try_from(raw).unwrap();
        let s = tm_hash.to_string();
        let restored = TmHash::from_str(&s).expect("TmHash::from_str failed");

        assert_eq!(tm_hash, restored);
    }

    #[test_strategy::proptest]
    fn proptest_str_roundtrip(raw: [u8; 32]) {
        test_str_roundtrip(raw);
    }

    #[test_strategy::proptest]
    fn proptest_serde_roundtrip(raw: [u8; 32]) {
        test_serialize_roundtrip(raw);
    }
}
