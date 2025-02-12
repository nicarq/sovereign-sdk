use std::time::Duration;

use schemars::JsonSchema;
use sov_rollup_interface::common::HexHash;
use sov_rollup_interface::da::Time;

use crate::storable::layer::StorableMockDaLayer;
use crate::{MockAddress, MockBlock, MockBlockHeader, MockHash};

/// Time in milliseconds to wait for the next block if it is not there yet.
/// How many times wait attempts are done depends on service configuration.
pub const WAIT_ATTEMPT_PAUSE: Duration = Duration::from_millis(10);
/// The max time for the requested block to be produced.
pub const DEFAULT_BLOCK_WAITING_TIME_MS: u64 = 120_000;

pub(crate) const GENESIS_HEADER: MockBlockHeader = MockBlockHeader {
    prev_hash: MockHash([0; 32]),
    hash: MockHash([1; 32]),
    height: 0,
    // 2023-01-01T00:00:00Z
    time: Time::from_secs(1672531200),
};

pub(crate) const GENESIS_BLOCK: MockBlock = MockBlock {
    header: GENESIS_HEADER,
    batch_blobs: Vec::new(),
    proof_blobs: Vec::new(),
};

/// Configuration for block producing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockProducingConfig {
    /// Blocks are produced at fixed time intervals, regardless of whether
    /// there are transactions. This means empty blocks may be created.
    Periodic {
        /// The interval, in milliseconds, at which new blocks are produced.
        block_time_ms: u64,
    },

    /// A new block is produced only when a batch blob (but not a proof blob) is submitted.
    /// Each block contains exactly one batch blob and zero or more proof blobs.
    OnBatchSubmit {
        /// The maximum time [`sov_rollup_interface::node::da::DaService::get_block_at`] will wait for a block to become available.
        /// If this timeout elapses, an error is returned.
        /// If set to `None`, [`DEFAULT_BLOCK_WAITING_TIME_MS`] is used.
        block_wait_timeout_ms: Option<u64>,
    },

    /// A new block is produced when either a batch blob or a proof blob is submitted.
    /// Each block contains exactly one blob.
    OnAnySubmit {
        /// The maximum time [`sov_rollup_interface::node::da::DaService::get_block_at`] will wait for a block to become available.
        /// If this timeout elapses, an error is returned.
        /// If set to `None`, [`DEFAULT_BLOCK_WAITING_TIME_MS`] is used.
        block_wait_timeout_ms: Option<u64>,
    },
    /// Blocks are created manually, with no automatic production.
    Manual,
}

/// What randomization we expect.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RandomizationBehaviour {
    /// Blobs inside a single blob are going to be out of order,
    /// but blobs will never pass the block boundary.
    OutOfOrderBlobs,
    /// Blobs in non-finalized blocks are going to be shuffled across all non-finalized blobs.
    /// The height of the chain is not going to be changed.
    ShuffleNonFinalizedBlobs {
        /// The percentage of blobs is going to be skipped forever.
        drop_percent: u8,
    },
    /// The height of the chain is going to be rewound to some height between last finalized and current.
    Rewind,
}

/// Configuration of randomization for non-finalized blocks.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize, JsonSchema)]
pub struct RandomizationConfig {
    /// Seed for Randomizer.
    pub seed: HexHash,
    /// What randomizer should do.
    pub behaviour: RandomizationBehaviour,
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
    /// Allow pointing to pre-existing [`StorableMockDaLayer`]
    #[serde(skip)]
    pub da_layer: Option<std::sync::Arc<tokio::sync::RwLock<StorableMockDaLayer>>>,
    /// If specified, [`StorableMockDaLayer`] will add randomization to non-finalized blocks.
    pub randomization: Option<RandomizationConfig>,
}

impl PartialEq for MockDaConfig {
    fn eq(&self, other: &Self) -> bool {
        let basic_eq = self.connection_string == other.connection_string
            && self.sender_address == other.sender_address
            && self.finalization_blocks == other.finalization_blocks
            && self.block_producing == other.block_producing
            && self.randomization == other.randomization;

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
    BlockProducingConfig::OnBatchSubmit {
        block_wait_timeout_ms: Some(DEFAULT_BLOCK_WAITING_TIME_MS),
    }
}

impl MockDaConfig {
    /// Create [`MockDaConfig`] with instant finality.
    pub fn instant_with_sender(sender: MockAddress) -> Self {
        MockDaConfig {
            connection_string: "sqlite::memory:".to_string(),
            sender_address: sender,
            finalization_blocks: 0,
            block_producing: default_block_producing(),
            da_layer: None,
            randomization: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_block_producing() {
        let config_s = r#"
            connection_string = "sqlite:///tmp/mockda.sqlite?mode=rwc"
            sender_address = "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f"
            finalization_blocks = 5
            [block_producing.periodic]
            block_time_ms = 1_000
        "#;
        let config = toml::from_str::<MockDaConfig>(config_s).unwrap();
        insta::assert_json_snapshot!(config);
    }

    #[test]
    fn manual_block_producing() {
        let config_s = r#"
            connection_string = "sqlite:///tmp/mockda.sqlite?mode=rwc"
            sender_address = "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f"
            [block_producing.manual]
        "#;
        let config = toml::from_str::<MockDaConfig>(config_s).unwrap();
        insta::assert_json_snapshot!(config);
    }

    #[test]
    fn with_randomization_shuffle() {
        let config_s = r#"
            connection_string = "sqlite:///tmp/mockda.sqlite?mode=rwc"
            sender_address = "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f"
            finalization_blocks = 5
            [block_producing.periodic]
            block_time_ms = 1_000
            [randomization]
            seed = "0x0000000000000000000000000000000000000000000000000000000000000012"
            [randomization.behaviour.shuffle_non_finalized_blobs]
            drop_percent = 0
        "#;
        let config = toml::from_str::<MockDaConfig>(config_s).unwrap();
        insta::assert_json_snapshot!(config);
    }

    #[test]
    fn with_randomization_rewind() {
        let config_s = r#"
            connection_string = "sqlite:///tmp/mockda.sqlite?mode=rwc"
            sender_address = "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f"
            finalization_blocks = 5
            [block_producing.periodic]
            block_time_ms = 1_000
            [randomization]
            seed = "0x0000000000000000000000000000000000000000000000000000000000000012"
            [randomization.behaviour.rewind]
        "#;
        let config = toml::from_str::<MockDaConfig>(config_s).unwrap();
        insta::assert_json_snapshot!(config);
    }
}
