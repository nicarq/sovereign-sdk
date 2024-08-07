use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_rollup_interface::da::DaSpec;

use crate::capabilities::FatalError;
use crate::transaction::SequencerReward;

/// RawTx represents a serialized rollup transaction received from the DA.
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub struct RawTx {
    /// Serialized transaction.
    pub data: Vec<u8>,
}

/// Contains raw transactions obtained from the DA blob.
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub struct Batch {
    /// Raw transactions.
    pub txs: Vec<RawTx>,
}

impl Batch {
    /// Construct a new batch containing the given txs.
    pub fn new(txs: Vec<RawTx>) -> Self {
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
    pub fn new_batch(txs: Vec<RawTx>) -> BlobData {
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

/// Represents the different outcomes that can occur for a sequencer after batch processing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BatchSequencerOutcome {
    /// Sequencer receives reward amount in defined token and can withdraw its deposit. The amount is net of any penalties.
    Rewarded(SequencerReward),
    /// Sequencer loses its deposit and receives no reward.
    Slashed(
        /// Reason why sequencer was slashed.
        FatalError,
    ),
    /// Batch was ignored, sequencer deposit left untouched.
    Ignored(
        /// Reason why the batch was ignored.
        String,
    ),
    /// The sequencer is not rewardable for the submitted batch.
    /// This occurs when an unregistered sequencer submits a batch directly to the DA.
    /// The batch might be applied but there is nobody to reward.
    NotRewardable,
}

/// A receipt for a batch that was submitted by a sequencer to the DA layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchSequencerReceipt<Da: DaSpec> {
    /// The da address of the sequencer that submitted the batch.
    pub da_address: Da::Address,
    /// The sequencer outcome for this batch.
    pub outcome: BatchSequencerOutcome,
}
