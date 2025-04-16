use std::env;
use std::path::PathBuf;
use std::time::Duration;

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
use testcontainers::core::{CmdWaitFor, ExecCommand, ExecResult, Host, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::io::AsyncBufReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

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
pub const VALIDATOR_METRICS_PORT: u16 = 9097;
/// Fixed Eth keys created by anvil. They don't change. Each address is funded 1000ETH
pub const ANVIL_ACCOUNTS: &[(&str, &str)] = &[
    (
        // First account is used by relayer, the rest belongs to validators
        "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    ),
    (
        "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
        "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    ),
    (
        "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc",
        "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    ),
    (
        "0x90f79bf6eb2c4f870365e785982e1f101e93b906",
        "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
    ),
    (
        "0x15d34aaf54267db7d7c367839aaf71a00a2c6a65",
        "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
    ),
    (
        "0x9965507d1a55bcc2695c58ba16fb37d819b0a4dc",
        "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
    ),
    (
        "0x976ea74026e726554db657fa54763abd0c3a0aa9",
        "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e",
    ),
    (
        "0x14dc79964da2c08b23698b3d3cc7ca32193d9955",
        "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356",
    ),
    (
        "0x23618e81e3f5cdf7f54c3d65f7fbc0abf5b21e8f",
        "0xdbda1821b80551c9d65939329250298aa3472ba22feea921c0cf5d620ea67b97",
    ),
    (
        "0xa0ee7a142d267c1f36714e4a8f75612f20a79720",
        "0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6",
    ),
];

pub struct Setup {
    pub sequencer: TestSequencer<TestSpec>,
    pub relayer: TestUser<TestSpec>,
    pub validators: Vec<TestUser<TestSpec>>,
    pub prover: TestProver<TestSpec>,
    pub genesis_config: GenesisConfig<TestSpec>,
}

pub fn generate_setup() -> Setup {
    let genesis_config =
        HighLevelZkGenesisConfig::generate_with_additional_accounts(ANVIL_ACCOUNTS.len());

    let relayer = genesis_config.additional_accounts[0].clone();
    let validators = (1..ANVIL_ACCOUNTS.len())
        .map(|n| genesis_config.additional_accounts[n].clone())
        .collect();
    let sequencer = genesis_config.initial_sequencer.clone();
    let prover = genesis_config.initial_prover.clone();

    let genesis_config = GenesisConfig::from_minimal_config(genesis_config.into(), (), (), ());

    Setup {
        sequencer,
        relayer,
        validators,
        prover,
        genesis_config,
    }
}

pub async fn setup_rollup(
    storage_path: PathBuf,
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
    pub validators: Vec<ExecResult>,
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
            validators: vec![],
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
            // map metrics ports to localhost
            .with_exposed_port(RELAYER_METRICS_PORT.tcp())
            .with_exposed_port(VALIDATOR_METRICS_PORT.tcp())
            // after this message relayer is ready
            .with_wait_for(WaitFor::message_on_stdout("Starting tokio console server"))
            // a bridge to the host system, to reach rollup from within container
            .with_host("host.docker.internal", Host::HostGateway)
            // default signing key for relayer
            // it's the first ethereum key created and reported by anvil
            // run `docker run --rm ghcr.io/eigerco/hyperlane anvil` to see all keys
            .with_env_var("HYP_KEY", ANVIL_ACCOUNTS[0].1)
            // setup agent config
            .with_copy_to("/agent-config.json", agent_config(rollup_port))
            .with_env_var("CONFIG_FILES", "/agent-config.json")
            // setup relayer keys
            .with_copy_to("/relayer-key.json", private_key)
            .with_env_var("TOKEN_KEY_FILE", "/relayer-key.json") // todo: rename this var in hyperlane
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

    /// Run Hyperlane validator in docker.
    pub async fn start_validator(&mut self, private_key: &PrivateKey) {
        let Some(container) = self.container.as_ref() else {
            panic!("called start_validator on not running container")
        };

        let private_key = json!({ "private_key": private_key });
        let private_key = serde_json::to_string(&private_key).unwrap();

        let val_id = self.validators.len();
        let val_key_file = format!("/validator{val_id}-key.json");
        let val_db_path = format!("/validator{val_id}/db");
        let val_sigs_path = format!("/validator{val_id}/signatures");

        // set the known port only for first validator, and let os choose random one for rest
        let metrics_port = if val_id == 0 {
            VALIDATOR_METRICS_PORT
        } else {
            0
        };

        let mkdir_cmd =
            ExecCommand::new(["mkdir", "-p", val_db_path.as_str(), val_sigs_path.as_str()]);
        container.exec(mkdir_cmd).await.unwrap();

        let upload_key_cmd = ExecCommand::new([
            "bash",
            "-c",
            format!("echo '{private_key}' > {val_key_file}").as_str(),
        ]);
        container.exec(upload_key_cmd).await.unwrap();

        let cmd = ExecCommand::new([
            // env vars for the validator
            "env",
            // location of the key for validator
            format!("TOKEN_KEY_FILE={val_key_file}").as_str(),
            // validator command
            "validator",
            // save signatures on local fs
            "--checkpointSyncer.type",
            "localStorage",
            // path to save signatures to, uses env var set in container
            "--checkpointSyncer.path",
            val_sigs_path.as_str(),
            // a database for validator storage
            "--db",
            val_db_path.as_str(),
            // a chain of which messages are going to be signed
            "--originChainName",
            "sovtest",
            // an eth key for the validator, reported by anvil
            "--validator.key",
            ANVIL_ACCOUNTS[val_id + 1].1,
            "--defaultSigner.type",
            "hexKey",
            "--defaultSigner.key",
            ANVIL_ACCOUNTS[val_id + 1].1,
            // port for metrics
            "--metrics-port",
            metrics_port.to_string().as_str(),
        ])
        .with_cmd_ready_condition(CmdWaitFor::message_on_stdout("Agent validator starting up"));

        let exec_result = container
            .exec(cmd)
            .await
            .expect("starting validator failed");
        self.validators.push(exec_result);
    }

    /// Prints container's stdout
    pub async fn print_stdout(&mut self) {
        let Some(container) = self.container.as_ref() else {
            return;
        };
        println!("RELAYER\n");
        let mut stdout = container.stdout(false).lines();
        while let Some(line) = stdout.next_line().await.unwrap() {
            println!("{line}");
        }

        // we don't have an option for no-follow stdout access
        // on `ExecResult`s, so this would hang infinitly waiting
        // for `exec`s to exit. Instead we give them at most 1s of
        // printing time each.
        for (n, val) in self.validators.iter_mut().enumerate() {
            println!("\n\nVALIDATOR {n}\n");

            let _ = timeout(Duration::from_secs(1), async {
                let mut stdout = val.stdout().lines();
                while let Some(line) = stdout.next_line().await.unwrap() {
                    println!("{line}");
                }
            })
            .await;
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
