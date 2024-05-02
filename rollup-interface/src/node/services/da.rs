//! The da module defines traits used by the full node to interact with the DA layer.

use alloc::vec::Vec;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::da::{BlockHeaderTrait, RelevantBlobs, RelevantProofs};
#[cfg(feature = "native")]
use crate::da::{DaSpec, DaVerifier};
use crate::zk::ValidityCondition;

/// Perform a checked arithmetic, returning None if the result is invalid.
pub trait CheckedMath<Rhs = Self> {
    /// The output type of arithmetic operations
    type Output;

    /// Performs checked multiplication, returning None if the result would overflow.
    fn checked_mul(&self, rhs: Rhs) -> Option<Self::Output>;
    /// Performs checked division, returning None if the caller attempts divide by zero or if the calculation overflows.
    fn checked_div(&self, rhs: Rhs) -> Option<Self::Output>;
    /// Performs checked subtraction, returning None if the result would underflow.
    fn checked_sub(&self, rhs: Rhs) -> Option<Self::Output>;
    /// Performs checked addition, returning None if the result would overflow.
    fn checked_add(&self, rhs: Rhs) -> Option<Self::Output>;
}

macro_rules! impl_checked_math_primitive {
    ($($t:ty),*) => {
        $(impl CheckedMath for $t {
            type Output = Self;

            fn checked_mul(&self, rhs: Self) -> Option<Self::Output> {
                <$t>::checked_mul(*self, rhs)
            }

            fn checked_div(&self, rhs: Self) -> Option<Self::Output> {
                <$t>::checked_div(*self, rhs)
            }

            fn checked_sub(&self, rhs: Self) -> Option<Self::Output> {
                <$t>::checked_sub(*self, rhs)
            }

            fn checked_add(&self, rhs: Self) -> Option<Self::Output> {
                <$t>::checked_add(*self, rhs)
            }
        })*
    };
}

impl_checked_math_primitive!(u8, u16, u32, u64, u128, usize);
impl_checked_math_primitive!(i8, i16, i32, i64, i128, isize);

/// The fee on a blockchain. This is usually expressed as a combination of a gas limit
/// and a fee rate (tokens per gas).
pub trait Fee {
    /// The price per unit of gas.
    type FeeRate: CheckedMath + CheckedMath<u64> + Clone + Send + Sync;

    /// Returns the price per unit of gas.
    fn fee_rate(&self) -> Self::FeeRate;

    /// Updates the price per unit of gas.
    fn set_fee_rate(&mut self, rate: Self::FeeRate);
}

/// A DaService is the local side of an RPC connection talking to a node of the DA layer
/// It is *not* part of the logic that is zk-proven.
///
/// The DaService has two responsibilities - fetching data from the DA layer, transforming the
/// data into a representation that can be efficiently verified in circuit.
#[cfg(feature = "native")]
#[async_trait::async_trait]
pub trait DaService: Send + Sync + 'static {
    /// A handle to the types used by the DA layer.
    type Spec: DaSpec;

    /// The verifier for this DA layer.
    type Verifier: DaVerifier<Spec = Self::Spec> + Clone;

    /// A DA layer block, possibly excluding some irrelevant information.
    type FilteredBlock: SlotData<
        BlockHeader = <Self::Spec as DaSpec>::BlockHeader,
        Cond = <Self::Spec as DaSpec>::ValidityCondition,
    >;

    /// Type that allow to consume [`futures::Stream`] of BlockHeaders.
    type HeaderStream: futures::Stream<Item = Result<<Self::Spec as DaSpec>::BlockHeader, Self::Error>>
        + Send;

    /// A transaction ID, used to identify the transaction in the DA layer.
    type TransactionId: PartialEq + Eq + PartialOrd + Ord + core::hash::Hash;

    /// The error type for fallible methods.
    type Error: core::fmt::Debug + Send + Sync + core::fmt::Display;

    /// The fee type for the DA layer.
    type Fee: Fee;

    /// Fetch the block at the given height, waiting for one to be mined if necessary.
    /// The returned block may not be final, and can be reverted without a consensus violation.
    /// Call it for the same height are allowed to return different results.
    /// Should always returns the block at that height on the best fork.
    async fn get_block_at(&self, height: u64) -> Result<Self::FilteredBlock, Self::Error>;

    /// Fetch the [`DaSpec::BlockHeader`] of the last finalized block.
    /// If there's no finalized block yet, it should return an error.
    async fn get_last_finalized_block_header(
        &self,
    ) -> Result<<Self::Spec as DaSpec>::BlockHeader, Self::Error>;

    /// Subscribe to finalized headers as they are finalized.
    /// Expect only to receive headers which were finalized after subscription
    /// Optimized version of `get_last_finalized_block_header`.
    async fn subscribe_finalized_header(&self) -> Result<Self::HeaderStream, Self::Error>;

    /// Fetch the head block of the most popular fork.
    ///
    /// More like utility method, to provide better user experience
    async fn get_head_block_header(
        &self,
    ) -> Result<<Self::Spec as DaSpec>::BlockHeader, Self::Error>;

    /// Extract the relevant transactions from a block. For example, this method might return
    /// all of the blob transactions from a set rollup namespaces on Celestia.
    fn extract_relevant_blobs(
        &self,
        block: &Self::FilteredBlock,
    ) -> RelevantBlobs<<Self::Spec as DaSpec>::BlobTransaction>;

    /// Generate a proof that the relevant blob transactions have been extracted correctly from the DA layer
    /// block.
    async fn get_extraction_proof(
        &self,
        block: &Self::FilteredBlock,
        blobs: &RelevantBlobs<<Self::Spec as DaSpec>::BlobTransaction>,
    ) -> RelevantProofs<
        <Self::Spec as DaSpec>::InclusionMultiProof,
        <Self::Spec as DaSpec>::CompletenessProof,
    >;

    /// Extract the relevant transactions from a block, along with a proof that the extraction has been done correctly.
    /// For example, this method might return all of the blob transactions in rollup's namespace on Celestia,
    /// together with a range proof against the root of the namespaced-merkle-tree, demonstrating that the entire
    /// rollup namespace has been covered.
    #[allow(clippy::type_complexity)]
    async fn extract_relevant_blobs_with_proof(
        &self,
        block: &Self::FilteredBlock,
    ) -> (
        RelevantBlobs<<Self::Spec as DaSpec>::BlobTransaction>,
        RelevantProofs<
            <Self::Spec as DaSpec>::InclusionMultiProof,
            <Self::Spec as DaSpec>::CompletenessProof,
        >,
    ) {
        let relevant_blobs = self.extract_relevant_blobs(block);
        let relevant_proofs = self.get_extraction_proof(block, &relevant_blobs).await;
        (relevant_blobs, relevant_proofs)
    }

    /// Send a transaction directly to the DA layer.
    /// blob is the serialized and signed transaction.
    /// Returns nothing if the transaction was successfully sent.
    async fn send_transaction(
        &self,
        blob: &[u8],
        fee: Self::Fee,
    ) -> Result<Self::TransactionId, Self::Error>;

    /// Sends am aggregated ZK proofs to the DA layer.
    async fn send_aggregated_zk_proof(
        &self,
        aggregated_proof_data: &[u8],
        fee: Self::Fee,
    ) -> Result<(), Self::Error>;

    /// Fetches all aggregated ZK proofs at a specified block height.
    async fn get_aggregated_proofs_at(&self, height: u64) -> Result<Vec<Vec<u8>>, Self::Error>;

    /// Estimates the appropriate fee for a blob with a given size
    async fn estimate_fee(&self, blob_size: usize) -> Result<Self::Fee, Self::Error>;
}

/// `SlotData` is the subset of a DA layer block which is stored in the rollup's database.
/// At the very least, the rollup needs access to the hashes and headers of all DA layer blocks,
/// but rollup may choose to store partial (or full) block data as well.
pub trait SlotData:
    Serialize + DeserializeOwned + PartialEq + core::fmt::Debug + Clone + Send + Sync
{
    /// The header type for a DA layer block as viewed by the rollup. This need not be identical
    /// to the underlying rollup's header type, but it must be sufficient to reconstruct the block hash.
    ///
    /// For example, most fields of the a Tendermint-based DA chain like Celestia are irrelevant to the rollup.
    /// For these fields, we only ever store their *serialized* representation in memory or on disk. Only a few special
    /// fields like `data_root` are stored in decoded form in the `CelestiaHeader` struct.
    type BlockHeader: BlockHeaderTrait;

    /// The validity condition associated with the slot data.
    type Cond: ValidityCondition;

    /// The canonical hash of the DA layer block.
    fn hash(&self) -> [u8; 32];
    /// The header of the DA layer block.
    fn header(&self) -> &Self::BlockHeader;
    /// Get the validity condition set associated with the slot
    fn validity_condition(&self) -> Self::Cond;
}
