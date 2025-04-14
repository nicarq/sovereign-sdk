use std::env;
use std::path::PathBuf;

use serde_json::json;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::macros::config_value;
use sov_modules_api::{CryptoSpec, Spec};
use sov_sequencer::preferred::PreferredSequencerConfig;
use sov_sequencer::SequencerKindConfig;
use sov_test_utils::runtime::genesis::zk::config::HighLevelZkGenesisConfig;
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, TestRollup};
use sov_test_utils::{RtAgnosticBlueprint, TestProver, TestSequencer, TestSpec, TestUser};
use testcontainers::core::client::docker_client_instance;
use testcontainers::core::{Host, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::io::AsyncBufReadExt;
use tokio::net::{TcpListener, TcpStream};

use super::preferred_sequencer_runtime::{GenesisConfig, TestRuntime};

pub type RollupBlueprint = RtAgnosticBlueprint<TestSpec, TestRuntime<TestSpec>>;
pub type TestRollupBuilder = RollupBuilder<RollupBlueprint, PathBuf>;
pub type PrivateKey = <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;

pub const DEFAULT_BLOCK_TIME_MS: u64 = 400;
pub const DEFAULT_BLOCK_PRODUCING_CONFIG: BlockProducingConfig = BlockProducingConfig::Periodic {
    block_time_ms: DEFAULT_BLOCK_TIME_MS,
};
pub const DEFAULT_FINALIZATION_BLOCKS: u32 = 10;
/// Use `container.get_host_port_ipv4(RELAYER_METRICS_PORT)` to get metrics
pub const RELAYER_METRICS_PORT: u16 = 9091;

pub struct Setup {
    pub sequencer: TestSequencer<TestSpec>,
    pub relayer: TestUser<TestSpec>,
    pub prover: TestProver<TestSpec>,
    pub genesis_config: GenesisConfig<TestSpec>,
}

pub fn generate_setup() -> Setup {
    let genesis_config = HighLevelZkGenesisConfig::generate_with_additional_accounts(2);

    let relayer = genesis_config.additional_accounts[0].clone();
    let sequencer = genesis_config.initial_sequencer.clone();
    let prover = genesis_config.initial_prover.clone();

    let genesis_config = GenesisConfig::from_minimal_config(genesis_config.into(), (), (), ());

    Setup {
        sequencer,
        relayer,
        prover,
        genesis_config,
    }
}

pub async fn setup_rollup(
    storage_path: PathBuf,
    axum_port: u16,
    setup: Setup,
) -> TestRollup<RollupBlueprint, PathBuf> {
    let rollup_builder = TestRollupBuilder::new_with_storage_path(
        GenesisSource::CustomParams(setup.genesis_config.clone().into_genesis_params()),
        DEFAULT_BLOCK_PRODUCING_CONFIG,
        DEFAULT_FINALIZATION_BLOCKS,
        storage_path,
    )
    .set_config(|config| {
        config.automatic_batch_production = true;
        config.rollup_prover_config = None;
        config.sequencer_config = SequencerKindConfig::Preferred(PreferredSequencerConfig {
            minimum_profit_per_tx: 0,
            ..Default::default()
        });
        config.prover_address = setup.prover.user_info.address().to_string();
        config.aggregated_proof_block_jump = 3;
        config.axum_port = axum_port;
    })
    .set_da_config(|da_config| {
        da_config.sender_address = setup.sequencer.da_address;
    });
    rollup_builder
        .start()
        .await
        .expect("Impossible to start rollup")
}

/// Helper for handling the dockerized hyperlane setup.
pub struct Hyperlane {
    pub image: GenericImage,
    pub container: Option<ContainerAsync<GenericImage>>,
}

impl Hyperlane {
    /// Sets up and pulls hyperlane image
    pub async fn new() -> Self {
        let docker_image = env::var("CUSTOM_HLP_DOCKER_IMAGE");
        let has_custom_image = !matches!(docker_image, Err(env::VarError::NotPresent));

        let docker_image =
            docker_image.unwrap_or_else(|_| "ghcr.io/eigerco/hyperlane:latest".into());
        let (name, tag) = docker_image
            .split_once(':')
            .unwrap_or((&docker_image, "latest"));

        let image = GenericImage::new(name, tag);

        // try to pull the image from registry before starting tests
        // but don't pull custom images, as they can be local and it would fail
        if !has_custom_image {
            let _ = image
                .clone()
                .pull_image()
                .await
                .expect("failed to pull image");
        }

        Self {
            image,
            container: None,
        }
    }

    /// Starts the container with hyperlane network
    pub async fn start(&mut self, private_key: &PrivateKey, rollup_port: u16) {
        let private_key = json!({ "private_key": private_key });
        let private_key = serde_json::to_vec(&private_key).unwrap();

        tokio::spawn(rollup_proxy(rollup_port));

        let container = self
            .image
            .clone()
            // map metrics port to localhost
            .with_exposed_port(RELAYER_METRICS_PORT.tcp())
            // after this message relayer is ready
            .with_wait_for(WaitFor::message_on_stdout("Starting tokio console server"))
            // a bridge to the host system, to reach rollup from within container
            .with_host("host.docker.internal", Host::HostGateway)
            // default signing key for relayer
            // it's the first ethereum key created and reported by anvil
            // run `docker run --rm ghcr.io/eigerco/hyperlane anvil` to see all keys
            .with_env_var(
                "HYP_KEY",
                "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            )
            // setup agent config
            .with_copy_to("/agent-config.json", agent_config(rollup_port))
            .with_env_var("CONFIG_FILES", "/agent-config.json")
            // setup relayer keys
            .with_copy_to("/relayer-key.json", private_key)
            .with_env_var("TOKEN_KEY_FILE", "/relayer-key.json") // todo: rename this var in hyperlane
            // place to look for validator signatures
            .with_env_var("VALIDATOR_SIGNATURES_DIR", "/validator-sigs")
            // relayer command
            .with_cmd([
                "relayer",
                "--db",
                "/relayer-db",
                "--relayChains",
                "sovtest",
                "--allowLocalCheckpointSyncers", // allow using validator signatures from local fs
                "true",
                "--metrics-port",
                RELAYER_METRICS_PORT.to_string().as_str(),
            ])
            .start()
            .await
            .expect("Failed starting hyperlane image");

        self.container = Some(container);
    }

    /// Prints container's stdout
    pub async fn print_stdout(&self) {
        let Some(container) = self.container.as_ref() else {
            return;
        };
        let mut stdout = container.stdout(false).lines();
        while let Some(line) = stdout.next_line().await.unwrap() {
            println!("{line}");
        }
    }
}

/// Generates a configuration file for the agents with the given rollup port
fn agent_config(rollup_port: u16) -> Vec<u8> {
    let config = json!({
        "chains": {
            "sovtest": {
                "chainId": config_value!("CHAIN_ID"),
                "displayName": "SovTest",
                "domainId": config_value!("HYPERLANE_BRIDGE_DOMAIN"),
                "isTestnet": true,
                "name": "sovtest",
                "nativeToken": {
                    "decimals": 18,
                    "name": "SovToken",
                    "symbol": "sov"
                },
                "protocol": "sovereign",
                "rpcUrls": [{
                    "http": format!("HTTP://host.docker.internal:{rollup_port}")
                }],
                // note: here we don't do much based on contract addresses, but some of those may
                // be needed to set to real addresses in a future
                "domainRoutingIsmFactory": "0x0000000000000000000000000000000000000000",
                "interchainAccountIsm": "0x0000000000000000000000000000000000000000",
                "interchainAccountRouter": "0x0000000000000000000000000000000000000000",
                "mailbox": "0x0000000000000000000000000000000000000000",
                "proxyAdmin": "0x0000000000000000000000000000000000000000",
                "staticAggregationHookFactory": "0x0000000000000000000000000000000000000000",
                "staticAggregationIsmFactory": "0x0000000000000000000000000000000000000000",
                "staticMerkleRootMultisigIsmFactory": "0x0000000000000000000000000000000000000000",
                "staticMerkleRootWeightedMultisigIsmFactory": "0x0000000000000000000000000000000000000000",
                "staticMessageIdMultisigIsmFactory": "0x0000000000000000000000000000000000000000",
                "staticMessageIdWeightedMultisigIsmFactory": "0x0000000000000000000000000000000000000000",
                "testRecipient": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "validatorAnnounce": "0x0000000000000000000000000000000000000000",
                "merkleTreeHook": "0x0000000000000000000000000000000000000000",
                "interchainGasPaymaster": "0x0000000000000000000000000000000000000000"
            }
            // An ethereum chain setup, to be used when writing end to end test between sov and evm
            // chain. May need some further tweaks.
            // "ethtest": {
            //     "chainId": 31337,
            //     "displayName": "EthTest",
            //     "domainId": 31337,
            //     "isTestnet": true,
            //     "name": "ethtest",
            //     "nativeToken": {
            //         "decimals": 18,
            //         "name": "Ether",
            //         "symbol": "ETH"
            //     },
            //     "protocol": "ethereum",
            //     "rpcUrls": [{
            //         "http": "HTTP://127.0.0.1:8545"
            //     }],
            //     "domainRoutingIsmFactory": "0xDc64a140Aa3E981100a9becA4E685f962f0cF6C9",
            //     "interchainAccountIsm": "0x9A676e781A523b5d0C0e43731313A708CB607508",
            //     "interchainAccountRouter": "0x68B1D87F95878fE05B998F19b66F4baba5De1aed",
            //     "mailbox": "0x8A791620dd6260079BF849Dc5567aDC3F2FdC318",
            //     "merkleTreeHook": "0xB7f8BC63BbcaD18155201308C8f3540b07f84F5e",
            //     "proxyAdmin": "0xa513E6E4b8f2a923D98304ec87F64353C4D5C853",
            //     "staticAggregationHookFactory": "0xCf7Ed3AccA5a467e9e704C703E8D87F634fB0Fc9",
            //     "staticAggregationIsmFactory": "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0",
            //     "staticMerkleRootMultisigIsmFactory": "0x5FbDB2315678afecb367f032d93F642f64180aa3",
            //     "staticMerkleRootWeightedMultisigIsmFactory": "0x5FC8d32690cc91D4c39d9d3abcBD16989F875707",
            //     "staticMessageIdMultisigIsmFactory": "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512",
            //     "staticMessageIdWeightedMultisigIsmFactory": "0x0165878A594ca255338adfa4d48449f69242Eb8F",
            //     "testRecipient": "0xc6e7DF5E7b4f2A278906862b61205850344D4e7d",
            //     "validatorAnnounce": "0x3Aa5ebB10DC797CAC828524e59A333d0A371443c",
            //     "interchainGasPaymaster": "0x0000000000000000000000000000000000000000",
            //     "index": {
            //         "from": 9
            //     }
            // }
        },
        "defaultRpcConsensusType": "fallback"
    });

    serde_json::to_vec(&config).unwrap()
}

/// Very simple proxy that listens on the docker's network interface and forwards traffic
/// between the rollup on localhost and the docker container
async fn rollup_proxy(rollup_port: u16) {
    let bridge_info = docker_client_instance()
        .await
        .unwrap()
        .inspect_network::<String>("bridge", None)
        .await
        .unwrap();
    let docker_gateway_ip = bridge_info
        .ipam
        .expect("no IPAM driver found")
        .config
        .expect("IPAM has no configuration")
        .into_iter()
        .find_map(|conf| conf.gateway)
        .expect("No gateway config in IPAM");

    // listen on the docker interface
    let listener = TcpListener::bind(format!("{docker_gateway_ip}:{rollup_port}"))
        .await
        .unwrap();

    loop {
        let (mut docker_socket, _) = listener.accept().await.unwrap();
        // connect to the rollup
        let Ok(mut rollup_socket) = TcpStream::connect(format!("127.0.0.1:{rollup_port}")).await
        else {
            // rollup shut down
            break;
        };

        // forward traffic between docker and rollup sockets
        tokio::spawn(async move {
            let _ = tokio::io::copy_bidirectional(&mut docker_socket, &mut rollup_socket).await;
        });
    }
}
