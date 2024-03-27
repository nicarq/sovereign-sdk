use std::path::Path;

use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::MockDaSpec;
use sov_modules_api::{DaSpec, Spec};
use sov_modules_stf_blueprint::{GenesisParams, StfBlueprint};
use sov_prover_storage_manager::ProverStorageManager;
use sov_state::DefaultStorageSpec;
use sov_stf_runner::read_json_file;

use crate::genesis_config::{get_genesis_config, GenesisPaths};
use crate::runtime::{GenesisConfig, Runtime};

mod da_simulation;
mod stf_tests;
mod tx_revert_tests;
pub(crate) type S = sov_test_utils::TestSpec;
pub(crate) type Da = MockDaSpec;

pub(crate) type RuntimeTest = Runtime<S, Da>;
pub(crate) type StfBlueprintTest =
    StfBlueprint<S, Da, sov_mock_zkvm::MockZkVerifier, RuntimeTest, BasicKernel<S, Da>>;

pub(crate) fn create_storage_manager_for_tests(
    path: impl AsRef<Path>,
) -> ProverStorageManager<MockDaSpec, DefaultStorageSpec> {
    let config = sov_state::config::Config {
        path: path.as_ref().to_path_buf(),
    };
    ProverStorageManager::new(config).unwrap()
}

pub(crate) fn get_genesis_config_for_tests<Da: DaSpec>(
) -> GenesisParams<GenesisConfig<S, Da>, BasicKernelGenesisConfig<S, Da>> {
    let integ_test_conf_dir: &Path = "../../test-data/genesis/integration-tests".as_ref();
    let rt_params =
        get_genesis_config::<S, Da>(&GenesisPaths::from_dir(integ_test_conf_dir)).unwrap();

    let chain_state = read_json_file(integ_test_conf_dir.join("chain_state.json")).unwrap();
    let kernel_params = BasicKernelGenesisConfig { chain_state };
    GenesisParams {
        runtime: rt_params,
        kernel: kernel_params,
    }
}

pub(crate) fn read_private_key<S: Spec>() -> PrivateKeyAndAddress<S> {
    let token_deployer_data =
        std::fs::read_to_string("../../test-data/keys/token_deployer_private_key.json")
            .expect("Unable to read file to string");

    let token_deployer: PrivateKeyAndAddress<S> = serde_json::from_str(&token_deployer_data)
        .unwrap_or_else(|_| {
            panic!(
                "Unable to convert data {} to PrivateKeyAndAddress",
                &token_deployer_data
            )
        });

    assert!(
        token_deployer.is_matching_to_default(),
        "Inconsistent key data"
    );

    token_deployer
}
