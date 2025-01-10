use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_rollup_interface::da::DaSpec;

use crate::{Context, DispatchCall, Gas, Runtime, Spec, StateCheckpoint, TxScratchpad};

/// FullyBakedTx represents a serialized signed rollup transaction that has been encoded with
/// authentication information and is ready to be placed on the DA layer.
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    BorshDeserialize,
    BorshSerialize,
    Serialize,
    Deserialize,
    derive_more::AsRef,
)]
pub struct FullyBakedTx {
    /// Serialized transaction.
    #[as_ref(forward)]
    pub data: Vec<u8>,
}

impl FullyBakedTx {
    /// Construct a FullyBakedTx containing the given data
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }
}

/// RawTx represents a serialized signed rollup transaction. A RawTx needs to be encoded
/// with authentication information before being placed on the DA layer.
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    BorshDeserialize,
    BorshSerialize,
    Serialize,
    Deserialize,
    derive_more::AsRef,
)]
pub struct RawTx {
    /// Serialized transaction.
    #[as_ref(forward)]
    pub data: Vec<u8>,
}

impl RawTx {
    /// Construct a RawTx containing the given data
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }
}

/// Contains raw transactions obtained from the DA blob.

/// A Batch with its ID.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BatchWithId {
    /// Batch of transactions.
    pub batch: Vec<FullyBakedTx>,
    /// The ID of the batch, carried over from the DA layer. This is the hash of the blob which contained the batch.
    pub id: [u8; 32],
}

impl BatchWithId {
    /// Construct a new batch with the given ID.
    pub fn new(batch: Vec<FullyBakedTx>, id: [u8; 32]) -> Self {
        Self { batch, id }
    }

    /// The size of all the transactions in the batch in bytes.
    pub fn batch_size(&self) -> usize {
        self.batch.iter().map(|tx| tx.data.len()).sum()
    }
}

/// Contains blob data obtained from the DA.
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobData {
    /// Batch of transactions.
    Batch(Vec<FullyBakedTx>),
    /// Emergency Registration
    EmergencyRegistration(RawTx),
    /// Aggregated proof posted on the DA.
    Proof(Vec<u8>),
}

impl BlobData {
    /// Tag the blob with the given ID.
    pub fn with_id(self, id: [u8; 32]) -> BlobDataWithId<BatchWithId> {
        match self {
            Self::Batch(batch) => BlobDataWithId::Batch(BatchWithId { batch, id }),
            Self::Proof(proof) => BlobDataWithId::Proof { proof, id },
            Self::EmergencyRegistration(tx) => BlobDataWithId::EmergencyRegistration { tx, id },
        }
    }
}

/// Contains blob data obtained from the DA.
//
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobDataWithId<B = IterableBatchWithId> {
    /// Batch of transactions.
    Batch(B),
    /// Emergency Registration
    EmergencyRegistration {
        /// The registration transaction
        tx: RawTx,
        /// The id of the blob on the DA layer
        id: [u8; 32],
    },
    /// Aggregated proof posted on the DA.
    Proof {
        /// The proof
        proof: Vec<u8>,
        /// The id of the blob on the DA layer
        id: [u8; 32],
    },
}

impl BlobDataWithId<BatchWithId> {
    /// The size of the blob in bytes.
    pub fn blob_size(&self) -> usize {
        match self {
            BlobDataWithId::Batch(b) => b.batch_size(),
            BlobDataWithId::Proof { proof, .. } => proof.len(),
            BlobDataWithId::EmergencyRegistration { tx, .. } => tx.data.len(),
        }
    }
}

/// The control flow to take after beginning to execute a tx.
pub enum TxControlFlow<R> {
    /// Continue processing the transaction using the provided output.
    ContinueProcessing(R),
    /// Ignore the tx, reverting any effects of its processing so far.
    IgnoreTx,
}

/// The provisional outcome for a sequencer after applying a single transaction
pub struct ProvisionalSequencerOutcome<R> {
    /// The sequencer's reward, in rollup tokens
    pub reward: u64,
    /// The sequencer's penalty, in rollup tokens
    pub penalty: u64,
    /// Whether the sequencer has run out of funds
    pub execution_status: MaybeExecuted<R>,
}

/// A transaction that may or may not have been executed
pub enum MaybeExecuted<R> {
    /// The execution result from the tx
    Executed(R),
    /// The transactions wasn't executed because the sequencer ran out of funds
    SequencerOutOfFunds,
}

impl<R> ProvisionalSequencerOutcome<R> {
    /// A convenient constructor for provisionally penalizing the sequencer and indicating
    /// that the sequencer has run out of funds.
    pub fn out_of_funds<G: Gas>(gas_used: &G, gas_price: &G::Price) -> Self {
        Self {
            reward: 0,
            penalty: gas_used.value(gas_price),
            execution_status: MaybeExecuted::SequencerOutOfFunds,
        }
    }

    /// A convenient constructor for provisionally penalizing the sequencer
    pub fn penalize<G: Gas>(gas_used: &G, gas_price: &G::Price, receipt: R) -> Self {
        Self {
            reward: 0,
            penalty: gas_used.value(gas_price),
            execution_status: MaybeExecuted::Executed(receipt),
        }
    }
    /// A convenient constructor for provisionally rewarding the sequencer
    pub fn reward(amount: u64, receipt: R) -> Self {
        Self {
            reward: amount,
            penalty: 0,
            execution_status: MaybeExecuted::Executed(receipt),
        }
    }
}

/// Allows the node component which produces a batch to inject logic at two points in transaction
/// lifecycle. This is used by the sequencer to unwind failing transactions and to inspect
/// the set of state changes made by a transaction before committing.
pub trait InjectedControlFlow<Receipt, S: Spec> {
    /// Runs after authentication but before the transaction executes
    fn pre_flight<RT: Runtime<S>>(
        &self,
        runtime: &RT,
        context: &Context<S>,
        call: &<RT as DispatchCall>::Decodable,
    ) -> TxControlFlow<()>;
    /// Runs after the transaction has executed.
    fn post_tx(
        &self,
        provisional_outcome: ProvisionalSequencerOutcome<Receipt>,
        dirty_scratchpad: TxScratchpad<S, StateCheckpoint<S>>,
    ) -> (StateCheckpoint<S>, TxControlFlow<Receipt>);
}

/// A batch that can be processed incrementally
pub trait IncrementalBatch<Receipt, S: Spec>:
    Iterator<Item = (FullyBakedTx, Self::ControlFlow)>
{
    /// The post tx hook type used by this funciton
    type ControlFlow: InjectedControlFlow<Receipt, S>;
    /// Returns an accurate lower bound on the remaining elements, if known.
    fn known_remaining_txs(&self) -> Option<usize>;

    /// The id of this blob on the DA layer, if known.
    fn id(&self) -> Option<[u8; 32]>;
}

impl<R, S: Spec> InjectedControlFlow<R, S> for NoOpControlFlow {
    fn pre_flight<RT: Runtime<S>>(
        &self,
        _runtime: &RT,
        _context: &Context<S>,
        _call: &<RT as DispatchCall>::Decodable,
    ) -> TxControlFlow<()> {
        TxControlFlow::ContinueProcessing(())
    }

    fn post_tx(
        &self,
        provisional_outcome: ProvisionalSequencerOutcome<R>,
        dirty_scratchpad: TxScratchpad<S, StateCheckpoint<S>>,
    ) -> (StateCheckpoint<S>, TxControlFlow<R>) {
        match provisional_outcome.execution_status {
            MaybeExecuted::Executed(receipt) => (
                dirty_scratchpad.commit(),
                TxControlFlow::ContinueProcessing(receipt),
            ),
            MaybeExecuted::SequencerOutOfFunds => {
                (dirty_scratchpad.commit(), TxControlFlow::IgnoreTx)
            }
        }
    }
}

/// Control flow which does not alter a transaction's execution path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoOpControlFlow;

/// A Batch with its ID.
#[derive(Debug)]

pub struct IterableBatchWithId {
    /// Batch of transactions.
    pub batch: std::vec::IntoIter<FullyBakedTx>,
    /// The ID of the batch, carried over from the DA layer. This is the hash of the blob which contained the batch.
    pub id: [u8; 32],
}

impl IterableBatchWithId {
    /// Create a new `IterableBatchWithId` from a `BatchWithId`.
    pub fn new(batch_with_id: BatchWithId) -> Self {
        Self {
            batch: batch_with_id.batch.into_iter(),
            id: batch_with_id.id,
        }
    }
}

impl Iterator for IterableBatchWithId {
    type Item = (FullyBakedTx, NoOpControlFlow);

    fn next(&mut self) -> Option<Self::Item> {
        self.batch.next().map(|tx| (tx, NoOpControlFlow))
    }
}

impl<Receipt, S: Spec> IncrementalBatch<Receipt, S> for IterableBatchWithId {
    type ControlFlow = NoOpControlFlow;

    fn known_remaining_txs(&self) -> Option<usize> {
        Some(self.batch.len())
    }

    fn id(&self) -> Option<[u8; 32]> {
        Some(self.id)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
/// The reason a transaction was rejected by the sequencer
pub enum RejectReason {
    /// The sender of the tx must be a sequencer admin
    SenderMustBeAdmin,
    /// The sequencer ran out of funds to pay for gas.
    SequencerOutOfGas,
    /// The transaction did not result in a sufficient reward.
    InsufficientReward {
        /// The minimum reward, in rollup tokens
        expected: u64,
        /// The reward received, in rollup tokens
        found: u64,
    },
}

impl BlobData {
    /// Batch variant constructor.
    pub fn new_batch(txs: Vec<FullyBakedTx>) -> Self {
        BlobData::Batch(txs)
    }

    /// Emergency Registration variant constructor.
    pub fn new_emergency_registration(tx: RawTx) -> Self {
        BlobData::EmergencyRegistration(tx)
    }

    /// Proof variant constructor.
    pub fn new_proof(proof: Vec<u8>) -> Self {
        BlobData::Proof(proof)
    }
}

impl<B> BlobDataWithId<B> {
    /// Convert the inner `Batch` type to another type, if applicable
    pub fn map_batch<Target>(self, f: impl FnOnce(B) -> Target) -> BlobDataWithId<Target> {
        match self {
            Self::Batch(b) => BlobDataWithId::Batch(f(b)),
            Self::Proof { proof, id } => BlobDataWithId::Proof { proof, id },
            Self::EmergencyRegistration { tx, id } => {
                BlobDataWithId::EmergencyRegistration { tx, id }
            }
        }
    }
}

/// The sequencer rewards.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, derive_more::Display,
)]
#[display(
    "Rewards {{ accumulated_reward:{}, accumulated_penalty: {} }}",
    accumulated_reward,
    accumulated_penalty
)]
pub struct Rewards {
    /// Rewards accumulated by the sequencer during the batch processing
    pub accumulated_reward: u64,
    /// Penalties accumulated by the sequencer during the batch processing
    pub accumulated_penalty: u64,
}

/// Outcome of batch execution.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, derive_more::Display,
)]
#[display("BatchSequencerOutcome {{ rewards: {} }}", rewards)]
#[serde(rename_all = "snake_case")]
pub struct BatchSequencerOutcome {
    /// Sequencer receives reward amount in defined token and can withdraw its deposit. The amount is net of any penalties.
    pub rewards: Rewards,
}

/// A receipt for a batch that was submitted by a sequencer to the DA layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, derive_more::Display)]
#[display(
    "BatchSequencerReceipt {{ da_address: {}, gas_price: {}, gas_used: {}, outcome: {} }}",
    da_address,
    gas_price,
    gas_used,
    outcome
)]
#[serde(bound = "S: Spec")]
pub struct BatchSequencerReceipt<S: Spec> {
    /// The da address of the sequencer that submitted the batch.
    pub da_address: <<S as Spec>::Da as DaSpec>::Address,
    /// Gas price for a given batch.
    pub gas_price: <S::Gas as Gas>::Price,
    /// Gas used during the batch execution.
    pub gas_used: S::Gas,
    /// The sequencer outcome for this batch.
    pub outcome: BatchSequencerOutcome,
}
