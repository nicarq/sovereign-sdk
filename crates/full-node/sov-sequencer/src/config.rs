use serde::{Deserialize, Serialize};
use sov_modules_api::schemars::JsonSchema;
use sov_modules_api::DaSpec;

use crate::batch_builders::preferred::PreferredBatchBuilderConfig;
use crate::batch_builders::standard::StdBatchBuilderConfig;

/// See [`SequencerConfig::batch_builder`].
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BatchBuilderConfig {
    /// A standard batch builder, which can post transactions to the rollup but not give soft confirmations.
    Standard(StdBatchBuilderConfig),
    /// A "Preferred" batch builder which is allowed to give soft confirmations.
    Preferred(PreferredBatchBuilderConfig),
}

impl Default for BatchBuilderConfig {
    fn default() -> Self {
        BatchBuilderConfig::Preferred(Default::default())
    }
}

/// Sequencer configuration.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[schemars(
    bound = "Da: DaSpec, Address: JsonSchema, BbConfig: JsonSchema",
    rename = "SequencerConfig"
)]
pub struct SequencerConfig<Da: DaSpec, Address, BbConfig = BatchBuilderConfig> {
    /// When enabled, submitted transactions are periodically assembled into
    /// batches and automatically posted to the DA layer. When disabled, the
    /// batch production endpoint has to be called explicitly.
    #[serde(default = "default_automatic_batch_production")]
    pub automatic_batch_production: bool,
    /// The sequencer won't process incoming requests unless the node is within
    /// this many blocks behind the DA chain head.
    pub max_allowed_blocks_behind: u64,
    /// For how many seconds the sequencer keeps track of dropped transactions
    /// after being done with them.
    ///
    /// Larger values result in higher memory usage, but better tx status
    /// tracking for users.
    #[serde(default = "default_sequencer_dropped_tx_ttl_secs")]
    pub dropped_tx_ttl_secs: u64,
    /// DA address of the sequencer.
    pub da_address: Da::Address,
    /// The list of addresses that are allowed to perform admin operations on
    /// the sequencer.
    // The custom "default" is equivalent to Serde's default default, but
    // without the bound `Address: Default`.
    #[serde(default = "Vec::<Address>::new")]
    pub admin_addresses: Vec<Address>,
    /// Batch builder configuration.
    #[serde(flatten)]
    pub batch_builder: BbConfig,
}

fn default_automatic_batch_production() -> bool {
    true
}

impl<Da: DaSpec, Addr: Clone, BbConfig> SequencerConfig<Da, Addr, BbConfig> {
    /// Replaces the value of [`SequencerConfig::batch_builder`].
    pub fn with_bb_config<BbConfig2>(
        &self,
        bb_config: BbConfig2,
    ) -> SequencerConfig<Da, Addr, BbConfig2> {
        SequencerConfig {
            automatic_batch_production: self.automatic_batch_production,
            dropped_tx_ttl_secs: self.dropped_tx_ttl_secs,
            da_address: self.da_address.clone(),
            max_allowed_blocks_behind: self.max_allowed_blocks_behind,
            admin_addresses: self.admin_addresses.clone(),
            batch_builder: bb_config,
        }
    }
}

impl<Da: DaSpec, Addr> SequencerConfig<Da, Addr> {
    /// Returns true if the batch builder uses [`BatchBuilderConfig::Preferred`].
    pub fn is_preferred_sequencer(&self) -> bool {
        matches!(self.batch_builder, BatchBuilderConfig::Preferred(_))
    }
}

fn default_sequencer_dropped_tx_ttl_secs() -> u64 {
    60
}
