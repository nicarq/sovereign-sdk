use serde::Deserialize;
use sov_modules_api::schemars::JsonSchema;
use sov_modules_api::DaSpec;

use crate::batch_builders::standard::StdBatchBuilderConfig;

/// See [`SequencerConfig::batch_builder`].
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct BatchBuilderConfig<Addr> {
    /// The list of addresses which can perform admin operations on the sequencer
    #[serde(default = "Vec::<Addr>::new")]
    pub admin_addresses: Vec<Addr>,
    /// The sequencer operation mode and its corresponding config.
    #[serde(flatten)]
    pub mode: BatchBuilderMode,
}

impl<S> BatchBuilderConfig<S> {
    /// Build a config for the standard sequencing mode with no admin addresses
    pub fn standard(config: StdBatchBuilderConfig) -> Self {
        Self {
            admin_addresses: Vec::new(),
            mode: BatchBuilderMode::Standard(config),
        }
    }

    /// Build a config for the preferred sequencing mode with no admin addresses
    pub fn preferred() -> Self {
        Self {
            admin_addresses: Vec::new(),
            mode: BatchBuilderMode::Preferred,
        }
    }
}

/// See [`SequencerConfig::batch_builder`].
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BatchBuilderMode {
    /// A standard batch builder, which can post transactions to the rollup but not give soft confirmations.
    Standard(StdBatchBuilderConfig),
    /// A "Preferred" batch builder which is allowed to give soft confirmations.
    Preferred,
}

impl<Addr> Default for BatchBuilderConfig<Addr> {
    fn default() -> Self {
        Self {
            admin_addresses: Vec::new(),
            mode: BatchBuilderMode::Standard(Default::default()),
        }
    }
}

/// Sequencer configuration.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[schemars(bound = "Da: DaSpec, BbConfig: JsonSchema", rename = "SequencerConfig")]
pub struct SequencerConfig<Da: DaSpec, BbConfig> {
    /// When enabled, submitted transactions are periodically assembled into
    /// batches and automatically posted to the DA layer. When disabled, the
    /// batch production endpoint has to be called explicitly.
    ///
    /// Experimental.
    // TODO(@neysofu): remove the experimental notice when we're confident it
    // works as expected.
    #[serde(default)]
    pub automatic_batch_production: bool,
    /// The sequencer won't process incoming requests unless the node is within
    /// this many blocks behind the DA chain head.
    pub max_allowed_blocks_behind: u64,
    /// How many long  the sequencer keeps track of dropped transactions after being done with them.
    ///
    /// Larger values result in higher memory usage, but better tx status
    /// tracking for users.
    #[serde(default = "default_sequencer_dropped_tx_ttl_secs")]
    pub dropped_tx_ttl_secs: u64,
    /// DA address of the sequencer.
    pub da_address: Da::Address,
    /// Batch builder configuration.
    #[serde(flatten)]
    pub batch_builder: BbConfig,
}

impl<Da: DaSpec, Addr> SequencerConfig<Da, BatchBuilderConfig<Addr>> {
    /// Returns true if the batch builder uses [`BatchBuilderMode::Preferred`].
    pub fn is_preferred_sequencer(&self) -> bool {
        self.batch_builder.mode == BatchBuilderMode::Preferred
    }
}

fn default_sequencer_dropped_tx_ttl_secs() -> u64 {
    60
}
