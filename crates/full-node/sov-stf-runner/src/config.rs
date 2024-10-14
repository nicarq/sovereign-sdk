use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sov_rollup_interface::node::da::DaService;
use sov_sequencer::SequencerConfig;

pub const DEFAULT_CONCURRENT_SYNC_TASKS: u8 = 5;

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
}

impl HttpServerConfig {
    /// Creates an [`HttpServerConfig`] that requests the operating system to bind to any available port.
    /// Useful for testing as it prevents multiple threads from binding to the same port.
    pub fn localhost_on_free_port() -> Self {
        HttpServerConfig {
            bind_host: "127.0.0.1".to_string(),
            bind_port: 0,
            public_address: None,
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
    /// The "distance"  measured in the number of blocks between two consecutive aggregated proofs.
    pub aggregated_proof_block_jump: usize,
    /// The prover receives rewards to this address.
    pub prover_address: Address,
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
    pub sequencer: SequencerConfig<Da::Spec>,
}

/// Reads toml file as a specific type.
pub fn from_toml_path<P: AsRef<Path>, R: DeserializeOwned>(path: P) -> anyhow::Result<R> {
    let mut contents = String::new();
    {
        let mut file = File::open(path)?;
        file.read_to_string(&mut contents)?;
    }
    tracing::debug!(
        size_in_bytes = contents.len(),
        contents,
        "Parsing config file"
    );

    let result: R = toml::from_str(&contents)?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;
    use std::str::FromStr;

    use sha2::Sha256;
    use sov_celestia_adapter::verifier::address::CelestiaAddress;
    use sov_celestia_adapter::CelestiaService;
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
            celestia_rpc_auth_token = "SECRET_RPC_TOKEN"
            celestia_rpc_address = "http://localhost:11111/"
            max_celestia_response_body_size = 980
            safe_lead_time_ms = 10
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
            [proof_manager]
            aggregated_proof_block_jump = 22
            prover_address = "sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94"
            [sequencer]
            max_allowed_blocks_behind = 5
            da_address = "celestia1a68m2l85zn5xh0l07clk4rfvnezhywc53g8x7s"
            [sequencer.standard]
        "#;

        let config_file = create_config_from(config);

        let config: RollupConfig<Address<Sha256>, CelestiaService> =
            from_toml_path(config_file.path()).unwrap();

        let expected = RollupConfig {
            runner: RunnerConfig {
                genesis_height: 31337,
                da_polling_interval_ms: 10_000,
                rpc_config: HttpServerConfig {
                    bind_host: "127.0.0.1".to_string(),
                    bind_port: 12345,
                    public_address: None,
                },
                axum_config: HttpServerConfig {
                    bind_host: "127.0.0.1".to_string(),
                    bind_port: 12346,
                    public_address: Some("https://rollup.sovereign.xyz".to_string()),
                },
                concurrent_sync_tasks: Some(18),
            },

            da: sov_celestia_adapter::CelestiaConfig {
                celestia_rpc_auth_token: "SECRET_RPC_TOKEN".to_string(),
                celestia_rpc_address: "http://localhost:11111/".into(),
                max_celestia_response_body_size: 980,
                celestia_rpc_timeout_seconds: 60,
                safe_lead_time_ms: 10,
            },
            storage: StorageConfig {
                path: PathBuf::from("/tmp"),
            },
            proof_manager: ProofManagerConfig {
                aggregated_proof_block_jump: 22,
                prover_address: Address::from_str(
                    "sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94",
                )
                .unwrap(),
            },
            sequencer: SequencerConfig {
                automatic_batch_production: false,
                max_allowed_blocks_behind: 5,
                dropped_tx_ttl_secs: 60,
                da_address: CelestiaAddress::from_str(
                    "celestia1a68m2l85zn5xh0l07clk4rfvnezhywc53g8x7s",
                )
                .unwrap(),
                batch_builder: BatchBuilderConfig::Standard(StdBatchBuilderConfig {
                    mempool_max_txs_count: None,
                    max_batch_size_bytes: None,
                }),
            },
        };
        assert_eq!(config, expected);
    }
}
