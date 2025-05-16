use celestia_types::consts::appconsts;
use celestia_types::nmt::{Namespace, NS_SIZE};
use prost::bytes::Buf;
use serde::{Deserialize, Serialize};
use sov_rollup_interface::Bytes;

/// The length of the "reserved bytes" field in a compact share
pub(crate) const RESERVED_BYTES_LEN: usize = appconsts::COMPACT_SHARE_RESERVED_BYTES;
/// The length of the "info byte" field in a compact share
pub(crate) const INFO_BYTE_LEN: usize = appconsts::SHARE_INFO_BYTES;
/// The length of the "sequence length" field
pub(crate) const SEQUENCE_LENGTH_LEN: usize = appconsts::SEQUENCE_LEN_BYTES;
/// The length of the "signer" field
// TODO: import constant once lumina updates their types.
pub(crate) const SIGNER_LEN: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum SovShare {
    Start(VersionedStartShare),
    Continuation(celestia_types::Share),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum VersionedStartShare {
    Zero(celestia_types::Share),
    One(celestia_types::Share),
}

impl AsRef<[u8]> for VersionedStartShare {
    fn as_ref(&self) -> &[u8] {
        match self {
            VersionedStartShare::Zero(inner) | VersionedStartShare::One(inner) => inner.as_ref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShareError {
    NotAStartShare,
    InvalidVersion,
    InvalidEncoding,
    InvalidSigner(String),
}

impl SovShare {
    pub(crate) fn new(inner: celestia_types::Share) -> anyhow::Result<Self> {
        if is_continuation_unchecked(inner.as_ref()) {
            Ok(Self::Continuation(inner))
        } else {
            let version = version_unchecked(inner.as_ref());
            Ok(match version {
                0 => Self::Start(VersionedStartShare::Zero(inner)),
                1 => Self::Start(VersionedStartShare::One(inner)),
                _ => anyhow::bail!(
                    "Unsupported share version {version}. Share must be version 0 or 1"
                ),
            })
        }
    }

    pub(crate) fn sequence_length(&self) -> Result<u64, ShareError> {
        match self {
            SovShare::Continuation(_) => Err(ShareError::NotAStartShare),
            SovShare::Start(VersionedStartShare::Zero(inner))
            | SovShare::Start(VersionedStartShare::One(inner)) => inner
                .sequence_length()
                .map(u64::from)
                .ok_or(ShareError::InvalidEncoding),
        }
    }

    pub(crate) fn blob_signer(
        &self,
    ) -> Result<crate::verifier::address::CelestiaAddress, ShareError> {
        match self {
            SovShare::Continuation(_) => Err(ShareError::NotAStartShare),
            SovShare::Start(start_share) => match start_share {
                VersionedStartShare::Zero(_) => Err(ShareError::InvalidVersion),
                VersionedStartShare::One(inner) => {
                    let start_idx = NS_SIZE + INFO_BYTE_LEN + SEQUENCE_LENGTH_LEN;
                    let end_idx = start_idx + SIGNER_LEN;

                    let share_bytes = inner.as_ref();
                    if share_bytes.len() < end_idx {
                        return Err(ShareError::InvalidSigner(
                            "Share is too short to contain a valid signer".to_string(),
                        ));
                    }
                    let signer_bytes = &share_bytes[start_idx..end_idx];
                    crate::verifier::address::CelestiaAddress::try_from(signer_bytes)
                        .map_err(|e| ShareError::InvalidSigner(e.to_string()))
                }
            },
        }
    }

    /// Returns this share in raw serialized form as a slice
    pub(crate) fn raw_inner_ref(&self) -> &[u8] {
        match self {
            SovShare::Continuation(inner) => inner.as_ref(),
            SovShare::Start(inner) => inner.as_ref(),
        }
    }

    /// When lumina adds support for share v1, can be removed and use [`celestia_types::Share::payload`].
    /// But for now it
    fn get_payload_offset(&self) -> usize {
        // All shares are prefixed with metadata including the namespace (8 bytes), and info byte (1 byte)
        let mut offset = NS_SIZE + INFO_BYTE_LEN;
        // Start shares are also prefixed with a sequence length
        if let Self::Start(version) = self {
            offset += SEQUENCE_LENGTH_LEN;
            if matches!(version, VersionedStartShare::One(_)) {
                offset += SIGNER_LEN;
            };
        }
        if self.namespace().is_reserved_on_celestia() {
            // Compact shares (shares in reserved namespaces) are prefixed with 4 reserved bytes
            offset += RESERVED_BYTES_LEN;
        }
        offset
    }

    /// Returns the data of this share as a `Bytes`
    pub fn payload(&self) -> Bytes {
        Bytes::copy_from_slice(self.payload_ref())
    }

    /// Returns the data of this share as &[u8]
    pub fn payload_ref(&self) -> &[u8] {
        &self.raw_inner_ref()[self.get_payload_offset()..]
    }

    /// Get the namespace associated with this share
    pub fn namespace(&self) -> Namespace {
        let out: [_; NS_SIZE] = self.raw_inner_ref()[..NS_SIZE]
            .try_into()
            .expect("can't fail for correct size");
        Namespace::from_raw(&out).expect("invalid namespace")
    }
}

impl AsRef<[u8]> for SovShare {
    fn as_ref(&self) -> &[u8] {
        match self {
            SovShare::Continuation(c) => c.as_ref(),
            SovShare::Start(s) => s.as_ref(),
        }
    }
}

fn is_continuation_unchecked(share: &[u8]) -> bool {
    share[NS_SIZE] & 0x01 == 0
}

fn version_unchecked(share: &[u8]) -> u8 {
    share[NS_SIZE] >> 1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Blob(pub Vec<SovShare>);

impl Blob {
    #[cfg(feature = "native")]
    pub(crate) fn signer(&self) -> Option<crate::verifier::address::CelestiaAddress> {
        match self.0[0].blob_signer() {
            Ok(signer) => Some(signer),
            Err(ShareError::InvalidVersion) => {
                tracing::warn!("Invalid share version for getting a signer");
                None
            }
            Err(e) => panic!(
                "Unexpected error when finding blob signer. This is a bug. Error: {:?}",
                e
            ),
        }
    }
}

/// Represents blob as a sequence of shares.
/// The first share in the `shares` vector should always be sequence start.
/// There can be only one such share.
/// Correct data block could be built from this struct.
#[cfg(feature = "native")]
pub(crate) struct ShareSequence {
    pub(crate) shares: Vec<celestia_types::Share>,
    // Range inside namespace, not data square.
    pub(crate) range_in_ns: std::ops::Range<usize>,
}

#[cfg(all(debug_assertions, feature = "native"))]
impl ShareSequence {
    pub(crate) fn check_consistency(&self) {
        assert!(
            !self.shares.is_empty(),
            "empty share seq does not make sense"
        );
        assert_eq!(
            self.shares.len(),
            self.range_in_ns.end - self.range_in_ns.start,
            "range/shares mismatch in share seq"
        );
        let first_share = &self.shares[0];
        let seq_len = first_share
            .sequence_length()
            .expect("first share must have sequence length");
        assert!(
            (seq_len as usize) < (self.shares.len() * appconsts::SHARE_SIZE),
            "sequence len {} won't fit into shares in this sequence {}",
            seq_len,
            self.shares.len(),
        );
        for idx in 1..self.shares.len() {
            let share = &self.shares[idx];
            assert_eq!(
                share.sequence_length(),
                None,
                "should be only one start share"
            );
        }
        let namespace = first_share.namespace();
        self.shares.iter().for_each(|share| {
            assert_eq!(
                share.namespace(),
                namespace,
                "All shares in sequence must have same namespace"
            );
        });
    }
}

#[cfg(feature = "native")]
impl TryFrom<ShareSequence> for Blob {
    type Error = anyhow::Error;

    fn try_from(value: ShareSequence) -> Result<Self, Self::Error> {
        let mut sov_shares = Vec::with_capacity(value.shares.len());
        for share in value.shares {
            let sov_share = SovShare::new(share)?;
            sov_shares.push(sov_share);
        }
        Ok(Self(sov_shares))
    }
}

impl IntoIterator for Blob {
    type Item = u8;

    type IntoIter = BlobIterator;

    fn into_iter(self) -> Self::IntoIter {
        let sequence_length = self.0[0]
            .sequence_length()
            .expect("blob must contain start share at idx 0");
        BlobIterator {
            sequence_len: sequence_length as usize,
            consumed: 0,
            current: self.0[0].payload(),
            current_idx: 0,
            blob: self,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlobIterator {
    sequence_len: usize,
    consumed: usize,
    current: Bytes,
    current_idx: usize,
    blob: Blob,
}

impl Iterator for BlobIterator {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.consumed == self.sequence_len {
            return None;
        }
        if self.current.has_remaining() {
            self.consumed += 1;
            return Some(self.current.get_u8());
        }
        self.current_idx += 1;
        self.current = self.blob.0[self.current_idx].payload();
        self.next()
    }
}

impl std::io::Read for BlobIterator {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut written = 0;
        for byte in buf.iter_mut() {
            *byte = match self.next() {
                Some(byte) => {
                    written += 1;
                    byte
                }
                None => return Ok(written),
            };
        }
        Ok(written)
    }
}

impl Buf for BlobIterator {
    fn remaining(&self) -> usize {
        self.sequence_len
            .checked_sub(self.consumed)
            .expect("BlobIterator has consumed more than available bytes")
    }

    fn chunk(&self) -> &[u8] {
        let chunk = if self.current.has_remaining() {
            self.current.as_ref()
        } else {
            // If the current share is exhausted, try to take the data from the next one
            // if there is no next chunk, we're done. Return the empty slice.
            if self.current_idx + 1 >= self.blob.0.len() {
                return &[];
            }
            // Otherwise, take the next chunk
            self.blob.0[self.current_idx + 1].payload_ref()
        };
        // Chunks are zero-padded, so truncate if necessary
        let remaining = self.remaining();
        if chunk.len() > remaining {
            return &chunk[..remaining];
        }
        chunk
    }

    fn advance(&mut self, mut cnt: usize) {
        self.consumed += cnt;
        while cnt > 0 {
            let remaining_in_current_share = self.current.remaining();
            if remaining_in_current_share > cnt {
                self.current.advance(cnt);
                return;
            } else {
                // Exhaust the current share
                self.current.advance(remaining_in_current_share);
                // If possible, advance the current share idx so that any future calls to `chunk` return
                // a non-empty array.
                if (self.current_idx + 1) < self.blob.0.len() {
                    self.current_idx += 1;
                    self.current = self.blob.0[self.current_idx].payload();
                } else {
                    // If advancing the current share was not possible, then we must have exactly used up the bytes
                    // in this blob. Assert that this is the case
                    assert_eq!(
                        remaining_in_current_share, cnt,
                        "Exhausted last share without fulfilling request!"
                    );
                }
                cnt -= remaining_in_current_share;
            }
        }
    }
}

/// Goes over namespace and splits it into blobs.
#[cfg(feature = "native")]
#[derive(Debug)]
pub(crate) struct NamespaceDataIterator<'a> {
    total_offset: usize,
    rows: &'a [celestia_types::row_namespace_data::RowNamespaceData],
    current_row_idx: Option<usize>,
    relative_share_idx: Option<usize>,
}

#[cfg(feature = "native")]
impl<'a> NamespaceDataIterator<'a> {
    #[cfg(feature = "native")]
    pub(crate) fn new(data: &'a celestia_types::row_namespace_data::NamespaceData) -> Self {
        let shares = data.rows.iter().map(|row| row.shares.len()).sum::<usize>();
        tracing::trace!(
            "Initialized NamespaceDataIterator: rows: {} shares: {}",
            data.rows.len(),
            shares
        );
        for (row_idx, row) in data.rows.iter().enumerate() {
            tracing::trace!("row {}: has {} shares", row_idx, row.shares.len());
        }
        NamespaceDataIterator {
            total_offset: 0,
            rows: &data.rows,
            current_row_idx: None,
            relative_share_idx: None,
        }
    }
}

#[cfg(feature = "native")]
impl<'a> Iterator for NamespaceDataIterator<'a> {
    type Item = ShareSequence;

    // Go over each row and each share, tracking offset.
    fn next(&mut self) -> Option<Self::Item> {
        tracing::trace!(
            "NamespaceDataIterator.next(). offset: {} current_row_idx: {:?}",
            self.total_offset,
            self.current_row_idx
        );
        if self.rows.is_empty() {
            // This can happen if the target namespace is empty. Then the row will
            // often contain two namespaces where the first is lower than the target and the second is larger.
            // In that case, the namespace root will "contain" the namespace, but no shares will be present.
            return None;
        }

        let mut row_idx = self.current_row_idx.unwrap_or(0);
        if row_idx >= self.rows.len() {
            return None;
        }
        let mut relative_share_idx = self.relative_share_idx.unwrap_or(0);

        let start = self.total_offset;
        let mut current_shares: Vec<celestia_types::Share> = Vec::new();

        while row_idx < self.rows.len() {
            let current_row = &self.rows[row_idx];

            while relative_share_idx < current_row.shares.len() {
                let share = &current_row.shares[relative_share_idx];
                let is_start = !is_continuation_unchecked(share.data());
                let is_tail_padding = is_tail_padding(share);
                // Found the new start. Stop and return all existing
                if is_start && !current_shares.is_empty() {
                    let range = start..self.total_offset;
                    self.current_row_idx = Some(row_idx);
                    return Some(ShareSequence {
                        shares: current_shares,
                        range_in_ns: range,
                    });
                }
                self.total_offset += 1;
                relative_share_idx += 1;
                self.relative_share_idx = Some(relative_share_idx);
                if !is_tail_padding {
                    current_shares.push(share.clone());
                }
            }
            row_idx += 1;
            self.current_row_idx = Some(row_idx);
            relative_share_idx = 0;
            self.relative_share_idx = Some(relative_share_idx);
        }

        self.current_row_idx = Some(self.rows.len());
        if !current_shares.is_empty() {
            // Return remaining
            let range = start..self.total_offset;
            Some(ShareSequence {
                shares: current_shares,
                range_in_ns: range,
            })
        } else {
            None
        }
    }
}

pub(crate) fn is_tail_padding(share: &celestia_types::Share) -> bool {
    share.sequence_length() == Some(0)
}

/// How many Celestia shares are needed to represent a payload of this size.
/// Celestia has two types of shares:
///  1. The first has extra metadata about the size of payload
///  2. Continuation shares have namespace and info bytes.
///
/// Technically, we rely on constants about size,
/// and it should be good as long as there are only two types of shares.
/// Copied from [`celestia_types::Blob::shares_len`]
pub(crate) fn shares_needed_for_bytes(payload_bytes: usize) -> usize {
    let Some(without_first_share) =
        payload_bytes.checked_sub(appconsts::FIRST_SPARSE_SHARE_CONTENT_SIZE)
    else {
        return 1;
    };
    1 + without_first_share.div_ceil(appconsts::CONTINUATION_SPARSE_SHARE_CONTENT_SIZE)
}

#[cfg(test)]
mod tests {
    use celestia_types::nmt::NS_ID_V0_SIZE;
    use celestia_types::row_namespace_data::NamespaceData;
    use proptest::collection::vec;
    use proptest::prelude::*;
    use sov_rollup_interface::da::CountedBufReader;

    use super::*;
    use crate::types::APP_VERSION;

    /// This test detects a regression, where we panic on trying to read the entire contents of a blob using the `Buf` trait.
    /// A previous implementation caused buf.advance(buf.remaining()) to panic because an internal state change of
    /// `current_share = blob.shares[blob.current_idx+1]` would be triggered as soon as the
    /// current share's data was read - even if the next share did not exist and would never be read.
    #[test]
    fn test_reading_full_blob_regression() {
        let data: NamespaceData = serde_json::from_str("[{\"shares\":[\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAc292LXRlc3QBAAAB3gEAAADWAQAA2EYzlAhN9KIdK3QP5WgAxmNMAiO3KOZd54PG1ndFjNud8Ij/Tn3TfZMkgVVIPduDBwKbg/S5/ShVT2cNHhn7CvitJDeieeHIkywHNYyR3E/jSGSpjGwl8pjioBmcFQn/UQEAAAIBSwEAAEZCB10foCn5JspyMO6m5QA8qYnV44fWffQU2E57kHzs4hKqnEjNwYnUJABmr6B1OJ/5+v7+BgPeOpxnaK4M5XovSUs8ExC7QtzrZxZT08DiU8nKCzK0VEUcDiu72Iyhnycjfxep9Nk7NuWt18/cz7YyUWYzlQcn6AN12JxkNF/Afx+DJzDJEzFE7bpzdjvFrl0im+L+wFF/T0IWonUXPL+oCvKoNDrDr+xA25mn9/rMDG/UGT0FI+KXHqx7c3ZR+5lLQheaxdVMRQcz2Mnst9uh8JKxVVVl9Fg4d7Jh6n2f9ZbBAfwg3YhW/5szA8YPKNg6o6nVnb90Cgk2RFVSOvxym4BijKId47SK6dfCS7G69iRFrDU+OcQTxsvkNb3mmnYa4TI9TGRHwl2N1yMHoJcWIboQPcLsKuykcJV+w4bvuB+uu8/pMVPjIDYAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\"],\"proof\":{\"start\":1,\"end\":2,\"nodes\":[\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABH1fFJqiHwbfG4vQXdK1Oh9vrkjlqWB365SYrsZxmaER\",\"/////////////////////////////////////////////////////////////////////////////yu8ipB5WivrLzIkx/iKuwyXvQFePPFTx9ZGUEhlxazq\"],\"leaf_hash\":\"\",\"is_max_namespace_ignored\":true}}]").unwrap();
        let ns_iterator = NamespaceDataIterator::new(&data);
        let blob: Blob = ns_iterator.into_iter().next().unwrap().try_into().unwrap();
        let mut blob = CountedBufReader::new(blob.into_iter());
        // Try to read the entire contents of the blob
        blob.advance(blob.total_len());
    }

    prop_compose! {
        fn share_bytes_strategy()(
            ns in vec(0u8.., NS_ID_V0_SIZE),
            mut share in vec(0u8.., appconsts::SHARE_SIZE),
            version in prop::bool::ANY, // Randomly choose between version 0 and 1
        ) -> Vec<u8> {
            let namespace = Namespace::new_v0(&ns).expect("doesn't exceed size");
            // overwrite namespace
            share[..NS_SIZE].copy_from_slice(namespace.as_ref());

            let version = if version {
                1u8
            } else {
                0u8
            };
            share[NS_SIZE] = (version << 1) | (share[NS_SIZE] & 0x01);

            share
        }
    }

    prop_compose! {
        fn blob_strategy()(
            ns in vec(0u8.., NS_ID_V0_SIZE),
            payload in vec(0u8.., 10_000),
            // Different version is not supported by Lumina now.
            // version in prop::bool::ANY,
        ) -> (celestia_types::Blob, Vec<u8>) {
            let namespace = Namespace::new_v0(&ns).expect("doesn't exceed size");
            // let version = if version {
            //     1u8
            // } else {
            //     0u8
            // };
            let blob = celestia_types::Blob::new(namespace, payload.clone(), APP_VERSION).unwrap();
            // blob.share_version = ;
            (blob, payload)
        }
    }

    #[test_strategy::proptest]
    fn proptest_share_serialization_deserialization(
        #[strategy(share_bytes_strategy())] data: Vec<u8>,
    ) {
        let bytes = celestia_types::Share::from_raw(&data).unwrap();
        let sov_share = SovShare::new(bytes).unwrap();
        let serialized = postcard::to_allocvec(&sov_share).unwrap();
        let deserialized: SovShare = postcard::from_bytes(&serialized).unwrap();
        prop_assert_eq!(sov_share, deserialized);
    }

    #[test_strategy::proptest]
    fn proptest_share_payload_test(
        #[strategy(blob_strategy())] input: (celestia_types::Blob, Vec<u8>),
    ) {
        let (blob, payload) = input;
        verify_share_payload(blob, payload);
    }

    fn verify_share_payload(blob: celestia_types::Blob, payload: Vec<u8>) {
        let shares = blob.to_shares().unwrap();
        let sov_shares = shares
            .into_iter()
            .map(SovShare::new)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        let mut reconstructed_payload = Vec::with_capacity(payload.len());

        for share in sov_shares {
            let share_data = share.payload_ref();
            reconstructed_payload.extend_from_slice(share_data);
        }

        // We allow reconstructed payload to be lagger, because of padding celestia adds.
        // This supposed to be handled by higher level structs
        assert!(reconstructed_payload.len() >= payload.len());
        assert_eq!(&reconstructed_payload[..payload.len()], &payload[..]);
    }

    #[test]
    #[ignore = "To be implemented"]
    fn share_payload_test() {
        let namespace = Namespace::new_v0(&[11u8; NS_ID_V0_SIZE]).unwrap();
        let payload = vec![11u8; 1200];
        let blob = celestia_types::Blob::new(namespace, payload.clone(), APP_VERSION).unwrap();
        verify_share_payload(blob, payload);
    }
}
