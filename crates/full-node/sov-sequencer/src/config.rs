use serde::{Deserialize, Serialize};
use sov_modules_api::schemars::JsonSchema;
use sov_modules_api::DaSpec;

use crate::preferred::PreferredSequencerConfig;
use crate::standard::StdSequencerConfig;

/// See [`SequencerConfig::sequencer_kind_config`].
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SequencerKindConfig {
    /// A "Standard" sequencer, which can post transactions to the rollup but not give soft confirmations.
    Standard(StdSequencerConfig),
    /// A "Preferred" sequencer which is allowed to give soft confirmations.
    Preferred(PreferredSequencerConfig),
}

impl Default for SequencerKindConfig {
    fn default() -> Self {
        SequencerKindConfig::Preferred(Default::default())
    }
}

/// Sequencer configuration.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[schemars(
    bound = "Da: DaSpec, Address: JsonSchema, Sc: JsonSchema",
    rename = "SequencerConfig"
)]
pub struct SequencerConfig<Da: DaSpec, Address, Sc = SequencerKindConfig> {
    /// When enabled, submitted transactions are periodically assembled into
    /// batches and automatically posted to the DA layer. When disabled, the
    /// batch production endpoint has to be called explicitly.
    #[serde(default = "default_automatic_batch_production")]
    pub automatic_batch_production: bool,
    /// The sequencer won't process incoming requests unless the node is within
    /// this many blocks or ahead of the sequencer.
    pub max_allowed_node_distance_behind: u64,
    /// For how many seconds the sequencer keeps track of dropped transactions
    /// after being done with them.
    ///
    /// Larger values result in higher memory usage, but better tx status
    /// tracking for users.
    #[serde(default = "default_sequencer_dropped_tx_ttl_secs")]
    pub dropped_tx_ttl_secs: u64,
    /// DA address of the sequencer.
    pub da_address: Da::Address,
    /// Rollup address of the sequencer.
    pub rollup_address: Address,
    /// The list of addresses that are allowed to perform admin operations on
    /// the sequencer.
    // The custom "default" is equivalent to Serde's default default, but
    // without the bound `Address: Default`.
    #[serde(default = "Vec::<Address>::new")]
    pub admin_addresses: Vec<Address>,
    /// Sequencer-type specific configuration.
    #[serde(flatten)]
    pub sequencer_kind_config: Sc,
    /// Maximum size of a batch.
    pub max_batch_size_bytes: usize,
    /// Maximum number of blobs sent in parallel.
    pub max_concurrent_blobs: usize,
}

fn default_automatic_batch_production() -> bool {
    true
}

impl<Da: DaSpec, Addr: Clone, BbConfig> SequencerConfig<Da, Addr, BbConfig> {
    /// Replaces the value of [`SequencerConfig::sequencer_kind_config`].
    pub fn with_seq_config<Sc2>(&self, seq_config: Sc2) -> SequencerConfig<Da, Addr, Sc2> {
        SequencerConfig {
            automatic_batch_production: self.automatic_batch_production,
            dropped_tx_ttl_secs: self.dropped_tx_ttl_secs,
            da_address: self.da_address.clone(),
            rollup_address: self.rollup_address.clone(),
            max_allowed_node_distance_behind: self.max_allowed_node_distance_behind,
            admin_addresses: self.admin_addresses.clone(),
            max_batch_size_bytes: self.max_batch_size_bytes,
            max_concurrent_blobs: self.max_concurrent_blobs,
            sequencer_kind_config: seq_config,
        }
    }
}

impl<Da: DaSpec, Addr> SequencerConfig<Da, Addr> {
    /// Returns true if the sequencer uses [`SequencerKindConfig::Preferred`].
    pub fn is_preferred_sequencer(&self) -> bool {
        matches!(
            self.sequencer_kind_config,
            SequencerKindConfig::Preferred(_)
        )
    }
}

fn default_sequencer_dropped_tx_ttl_secs() -> u64 {
    60
}
