use std::num::NonZero;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sov_rollup_interface::node::da::DaService;
use sov_sequencer::{BatchBuilderConfig, SequencerConfig};

pub const DEFAULT_CONCURRENT_SYNC_TASKS: u8 = 5;
pub use sov_metrics::MonitoringConfig;

/// Configuration for StateTransitionRunner.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
pub struct RunnerConfig {
    /// DA start height.
    pub genesis_height: u64,
    /// Polling interval for the DA service to check the sync status (in milliseconds).
    pub da_polling_interval_ms: u64,
    /// RPC configuration.
    pub rpc_config: HttpServerConfig,
    /// Axum server configuration.
    pub axum_config: HttpServerConfig,
    /// How many concurrent tasks to get block from DA service
    pub concurrent_sync_tasks: Option<u8>,
}

impl RunnerConfig {
    pub(crate) fn get_concurrent_sync_tasks(&self) -> u8 {
        self.concurrent_sync_tasks
            .unwrap_or(DEFAULT_CONCURRENT_SYNC_TASKS)
    }
}

/// Configuration for HTTP server(s) exposed by the node.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
pub struct HttpServerConfig {
    /// Server host.
    pub bind_host: String,
    /// Server port.
    pub bind_port: u16,
    /// The fully qualified public name of the server, in case the rollup node is running behind a proxy.
    /// For instance:
    /// ```toml
    /// public_address = "https://rollup.example.com"
    /// ```
    pub public_address: Option<String>,
    /// Enable or disable CORS policy headers. Enabled by default.
    #[serde(default)]
    pub cors: CorsConfiguration,
}

/// See [`HttpServerConfig::cors`].
///
/// # Default
///
/// CORS makes local development easier, so it's enabled by default.
///
/// In production, one may want to disable it and let your reverse proxy
/// handle CORS instead.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CorsConfiguration {
    /// Enables CORS with a permissive policy, intended for local development.
    #[default]
    Enabled,
    /// Disables CORS.
    Disabled,
}

impl HttpServerConfig {
    /// Creates an [`HttpServerConfig`] that requests the operating system to bind to any available port.
    /// Useful for testing as it prevents multiple threads from binding to the same port.
    pub fn localhost_on_free_port() -> Self {
        HttpServerConfig {
            bind_host: "127.0.0.1".to_string(),
            bind_port: 0,
            public_address: None,
            cors: CorsConfiguration::Enabled,
        }
    }
}

/// Simple storage configuration
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
pub struct StorageConfig {
    /// Path that can be utilized by concrete implementation
    pub path: PathBuf,
}

/// Prover service configuration.
#[derive(Debug, Clone, PartialEq, Deserialize, Copy, JsonSchema)]
pub struct ProofManagerConfig<Address> {
    /// The "distance" measured in the number of blocks between two consecutive aggregated proofs.
    pub aggregated_proof_block_jump: NonZero<usize>,
    /// The prover receives rewards to this address.
    pub prover_address: Address,
    /// A number of state transition info entries are allowed to be stored in the database.
    /// When the number is exceeded, older entries are removed.
    pub max_number_of_transitions_in_db: NonZero<u64>,
    /// A number of state transition info entries are allowed to be kept in memory.
    /// If the number is exceeded, rollup execution will be blocked until provers cathes up.
    pub max_number_of_transitions_in_memory: NonZero<u64>,
}

/// Rollup Configuration
#[derive(Debug, Clone, Deserialize, JsonSchema, derivative::Derivative)]
#[derivative(PartialEq(bound = "Address: PartialEq, Da: DaService"))]
#[schemars(bound = "Address: JsonSchema, Da: DaService", rename = "RollupConfig")]
pub struct RollupConfig<Address, Da: DaService> {
    /// Currently rollup config runner only supports storage path parameter
    pub storage: StorageConfig,
    /// Runner own configuration.
    pub runner: RunnerConfig,
    /// Data Availability service configuration.
    pub da: Da::Config,
    /// Proof manager configuration.
    pub proof_manager: ProofManagerConfig<Address>,
    /// Sequencer (and batch builder) configuration.
    pub sequencer: SequencerConfig<Da::Spec, Address, BatchBuilderConfig>,
    /// Monitoring configuration.
    pub monitoring: MonitoringConfig,
}

/// Reads toml file as a specific type.
pub fn from_toml_path<P: AsRef<Path>, R: DeserializeOwned>(path: P) -> anyhow::Result<R> {
    let contents = std::fs::read_to_string(&path)?;

    tracing::info!(
        path = path.as_ref().to_string_lossy().to_string(),
        size_in_bytes = contents.len(),
        line_count = contents.lines().count(),
        "Parsing rollup configuration file"
    );

    Ok(toml::from_str(&contents)?)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;
    use std::str::FromStr;

    use sha2::Sha256;
    use sov_mock_da::{MockAddress, MockDaService};
    use sov_modules_api::Address;
    use sov_sequencer::batch_builders::standard::StdBatchBuilderConfig;
    use sov_sequencer::{BatchBuilderConfig, SequencerConfig};
    use tempfile::NamedTempFile;

    use super::*;

    fn create_config_from(content: &str) -> NamedTempFile {
        let mut config_file = NamedTempFile::new().unwrap();
        config_file.write_all(content.as_bytes()).unwrap();
        config_file
    }

    #[test]
    fn test_correct_config() {
        let config = r#"
            [da]
            connection_string = "sqlite:///tmp/mockda.sqlite?mode=rwc"
            sender_address = "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f"
            block_producing = "periodic"
            block_time_ms = 1_000
            [storage]
            path = "/tmp"
            [runner]
            genesis_height = 31337
            da_polling_interval_ms = 10000
            concurrent_sync_tasks = 18
            [runner.rpc_config]
            bind_host = "127.0.0.1"
            bind_port = 12345
            [runner.axum_config]
            bind_host = "127.0.0.1"
            bind_port = 12346
            public_address = "https://rollup.sovereign.xyz"
            cors = "disabled"
            [monitoring]
            telegraf_address = "192.168.4.5:8543"
            max_datagram_size = 1024
            max_pending_metrics = 2560
            [proof_manager]
            aggregated_proof_block_jump = 22
            prover_address = "sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7qhzze66"
            max_number_of_transitions_in_db = 1025
            max_number_of_transitions_in_memory = 768
            [sequencer]
            max_allowed_blocks_behind = 5
            da_address = "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f"
            [sequencer.standard]
        "#;

        let config_file = create_config_from(config);

        let config: RollupConfig<Address<Sha256>, MockDaService> =
            from_toml_path(config_file.path()).unwrap();

        let expected = RollupConfig {
            runner: RunnerConfig {
                genesis_height: 31337,
                da_polling_interval_ms: 10_000,
                rpc_config: HttpServerConfig {
                    bind_host: "127.0.0.1".to_string(),
                    bind_port: 12345,
                    public_address: None,
                    cors: CorsConfiguration::Enabled,
                },
                axum_config: HttpServerConfig {
                    bind_host: "127.0.0.1".to_string(),
                    bind_port: 12346,
                    public_address: Some("https://rollup.sovereign.xyz".to_string()),
                    cors: CorsConfiguration::Disabled,
                },
                concurrent_sync_tasks: Some(18),
            },

            da: sov_mock_da::MockDaConfig {
                connection_string: "sqlite:///tmp/mockda.sqlite?mode=rwc".to_string(),
                sender_address: MockAddress::new([15; 32]),
                finalization_blocks: 0,
                block_producing: sov_mock_da::BlockProducingConfig::Periodic,
                block_time_ms: 1000,
            },
            storage: StorageConfig {
                path: PathBuf::from("/tmp"),
            },
            proof_manager: ProofManagerConfig {
                aggregated_proof_block_jump: NonZero::new(22).unwrap(),
                prover_address: Address::from_str(
                    "sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7qhzze66",
                )
                .unwrap(),
                max_number_of_transitions_in_db: NonZero::new(1025).unwrap(),
                max_number_of_transitions_in_memory: NonZero::new(768).unwrap(),
            },
            sequencer: SequencerConfig {
                automatic_batch_production: false,
                admin_addresses: vec![],
                max_allowed_blocks_behind: 5,
                dropped_tx_ttl_secs: 60,
                da_address: MockAddress::from_str(
                    "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f",
                )
                .unwrap(),
                batch_builder: BatchBuilderConfig::Standard(StdBatchBuilderConfig {
                    mempool_max_txs_count: None,
                    max_batch_size_bytes: None,
                }),
            },
            monitoring: MonitoringConfig {
                telegraf_address: std::net::SocketAddr::from_str("192.168.4.5:8543").unwrap(),
                max_datagram_size: Some(1024),
                max_pending_metrics: Some(2560),
            },
        };
        assert_eq!(config, expected);
    }
}
