use std::time::Duration;

use schemars::JsonSchema;
use sov_rollup_interface::da::Time;

use crate::storable::layer::StorableMockDaLayer;
use crate::storable::service::BlockProducing;
use crate::{MockAddress, MockBlock, MockBlockHeader, MockHash, MockValidityCond};

/// Time in milliseconds to wait for the next block if it is not there yet.
/// How many times wait attempts are done depends on service configuration.
pub const WAIT_ATTEMPT_PAUSE: Duration = Duration::from_millis(10);

pub(crate) const GENESIS_HEADER: MockBlockHeader = MockBlockHeader {
    prev_hash: MockHash([0; 32]),
    hash: MockHash([1; 32]),
    height: 0,
    // 2023-01-01T00:00:00Z
    time: Time::from_secs(1672531200),
};

pub(crate) const GENESIS_BLOCK: MockBlock = MockBlock {
    header: GENESIS_HEADER,
    validity_cond: MockValidityCond { is_valid: true },
    batch_blobs: Vec::new(),
    proof_blobs: Vec::new(),
};

/// Configuration for block producing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockProducingConfig {
    /// New blocks are produced periodically.
    /// This means that empty blocks can be produced.
    Periodic,
    /// New blocks are produced only when a batch blob is submitted, not proof.
    /// This also means that the block has only one blob.
    OnBatchSubmit,
    /// New blocks are produced only when batch or proof blobs are submitted.
    /// This also means that the block has only one blob.
    OnAnySubmit,
    /// Blocks produced by hand.
    Manual,
}

/// The configuration for Mock Da.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, JsonSchema)]
pub struct MockDaConfig {
    /// Connection string to the database for storing Da Data.
    ///   - "sqlite://demo_data/da.sqlite?mode=rwc"
    ///   - "sqlite::memory:"
    ///   - "postgresql://root:hunter2@aws.amazon.com/mock-da"
    pub connection_string: String,
    /// The address to use to "submit" blobs on the mock da layer.
    pub sender_address: MockAddress,
    /// Defines how many blocks progress to finalization.
    #[serde(default)]
    pub finalization_blocks: u32,
    /// How MockDaService should produce blocks.
    #[serde(default = "default_block_producing")]
    pub block_producing: BlockProducingConfig,
    /// Block time depends on `block_producing`:
    ///  - For [`BlockProducingConfig::Periodic`] it defines how often new blocks will be produced, approximately.
    ///  - For [`BlockProducingConfig::OnBatchSubmit`] or [`BlockProducingConfig::OnAnySubmit`] it defines max time service will wait for a new block to be submitted.
    #[serde(default = "default_block_time_ms")]
    pub block_time_ms: u64,
    /// Allow pointing to pre-existing [`StorableMockDaLayer`]
    #[serde(skip)]
    pub da_layer: Option<std::sync::Arc<tokio::sync::RwLock<StorableMockDaLayer>>>,
}

impl PartialEq for MockDaConfig {
    fn eq(&self, other: &Self) -> bool {
        let basic_eq = self.connection_string == other.connection_string
            && self.sender_address == other.sender_address
            && self.finalization_blocks == other.finalization_blocks
            && self.block_producing == other.block_producing
            && self.block_time_ms == other.block_time_ms;

        // Basic fields are not equal, no need to check da_layer field
        if !basic_eq {
            false
        } else {
            // We can only consider them Eq if `DaLayer` is None in both cases
            self.da_layer.is_none() && other.da_layer.is_none()
        }
    }
}

pub(crate) fn default_block_producing() -> BlockProducingConfig {
    BlockProducingConfig::OnBatchSubmit
}

pub(crate) fn default_block_time_ms() -> u64 {
    120_000
}

impl MockDaConfig {
    /// Create [`MockDaConfig`] with instant finality.
    pub fn instant_with_sender(sender: MockAddress) -> Self {
        MockDaConfig {
            connection_string: "sqlite::memory:".to_string(),
            sender_address: sender,
            finalization_blocks: 0,
            block_producing: default_block_producing(),
            block_time_ms: default_block_time_ms(),
            da_layer: None,
        }
    }

    pub(crate) fn block_producing(&self) -> BlockProducing {
        match self.block_producing {
            BlockProducingConfig::Periodic => {
                BlockProducing::Periodic(Duration::from_millis(self.block_time_ms))
            }
            BlockProducingConfig::OnBatchSubmit => {
                BlockProducing::OnBatchSubmit(Duration::from_millis(self.block_time_ms))
            }
            BlockProducingConfig::OnAnySubmit => {
                BlockProducing::OnAnySubmit(Duration::from_millis(self.block_time_ms))
            }
            BlockProducingConfig::Manual => BlockProducing::Manual,
        }
    }
}
