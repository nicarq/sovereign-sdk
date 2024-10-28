use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_rollup_interface::da::DaSpec;

use crate::{Gas, Spec};

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
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub struct Batch {
    /// Raw transactions.
    pub txs: Vec<FullyBakedTx>,
}

impl Batch {
    /// Construct a new batch containing the given txs.
    pub fn new(txs: Vec<FullyBakedTx>) -> Self {
        Self { txs }
    }
}

/// Batch with ID.
pub struct BatchWithId {
    /// Batch of transactions.
    pub batch: Batch,
    /// The ID of the batch, carried over from the DA layer. This is the hash of the blob which contained the batch.
    pub id: [u8; 32],
}

/// Contains blob data obtained from the DA.
//
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobData {
    /// Batch of transactions.
    Batch(Batch),
    /// Emergency Registration
    EmergencyRegistration(RawTx),
    /// Aggregated proof posted on the DA.
    Proof(Vec<u8>),
}

impl BlobData {
    /// Batch variant constructor.
    pub fn new_batch(txs: Vec<FullyBakedTx>) -> BlobData {
        BlobData::Batch(Batch { txs })
    }

    /// Emergency Registration variant constructor.
    pub fn new_emergency_registration(tx: RawTx) -> BlobData {
        BlobData::EmergencyRegistration(tx)
    }

    /// Proof variant constructor.
    pub fn new_proof(proof: Vec<u8>) -> BlobData {
        BlobData::Proof(proof)
    }
}

/// Contains blob data obtained from the DA blob together with the ID of the blob.
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub struct BlobDataWithId {
    /// Raw transactions.
    pub data: BlobData,
    /// The ID of the batch, carried over from the DA layer. This is the hash of the blob which contained the batch.
    pub id: [u8; 32],
}

/// The sequencer rewards.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rewards {
    /// Rewards accumulated by the sequencer during the batch processing
    pub accumulated_reward: u64,
    /// Penalties accumulated by the sequencer during the batch processing
    pub accumulated_penalty: u64,
    /// Costs of executing the batch hooks.
    pub hooks_cost: u64,
}

/// Represents the different outcomes that can occur for a sequencer after batch processing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchSequencerOutcome {
    /// Sequencer receives reward amount in defined token and can withdraw its deposit. The amount is net of any penalties.
    Executed(Rewards),
    /// Batch was ignored, sequencer deposit left untouched.
    Ignored(
        /// Reason why the batch was ignored.
        String,
    ),
}

/// A receipt for a batch that was submitted by a sequencer to the DA layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
