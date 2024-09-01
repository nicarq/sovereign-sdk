use std::path::Path;

use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockAddress, MockBlob, MockDaSpec};
use sov_modules_api::{Batch, DaSpec, RawTx, Spec};
use sov_modules_stf_blueprint::{BatchReceipt, GenesisParams, StfBlueprint};

use crate::genesis_config::{create_genesis_config, GenesisPaths};
use crate::runtime::{GenesisConfig, Runtime};

mod da_simulation;
mod stf_tests;
mod tx_revert_tests;

pub(crate) type S = sov_test_utils::TestSpec;
pub(crate) type Da = MockDaSpec;
pub(crate) type RuntimeTest = Runtime<S, Da>;
pub(crate) type StfBlueprintTest = StfBlueprint<S, Da, RuntimeTest, BasicKernel<S, Da>>;

pub(crate) struct TestPrivateKeys<S: Spec> {
    pub token_deployer: PrivateKeyAndAddress<S>,
    pub tx_signer: PrivateKeyAndAddress<S>,
}

pub(crate) fn create_genesis_config_for_tests<Da: DaSpec>(
) -> GenesisParams<GenesisConfig<S, Da>, BasicKernelGenesisConfig<S, Da>> {
    let integ_test_conf_dir: &Path = "../../test-data/genesis/stf-tests".as_ref();
    let rt_params =
        create_genesis_config::<S, Da>(&GenesisPaths::from_dir(integ_test_conf_dir)).unwrap();

    let kernel_params = BasicKernelGenesisConfig::from_path(
        Path::new(integ_test_conf_dir).join("chain_state.json"),
    )
    .unwrap();
    GenesisParams {
        runtime: rt_params,
        kernel: kernel_params,
    }
}

const PRIVATE_KEYS_DIR: &str = "../../test-data/keys";

fn read_and_parse_private_key<S: Spec>(suffix: &str) -> PrivateKeyAndAddress<S> {
    PrivateKeyAndAddress::from_json_file(Path::new(PRIVATE_KEYS_DIR).join(suffix)).unwrap()
}

pub(crate) fn read_private_keys<S: Spec>() -> TestPrivateKeys<S> {
    let token_deployer = read_and_parse_private_key::<S>("token_deployer_private_key.json");
    let tx_signer = read_and_parse_private_key::<S>("tx_signer_private_key.json");

    TestPrivateKeys::<S> {
        token_deployer,
        tx_signer,
    }
}

/// Builds a [`MockBlob`] from a [`Batch`] and a given address.
pub fn new_test_blob_from_batch(
    batch: Batch,
    address: &[u8],
    hash: [u8; 32],
) -> <MockDaSpec as DaSpec>::BlobTransaction {
    let address = MockAddress::try_from(address).unwrap();
    let data = borsh::to_vec(&batch).unwrap();
    MockBlob::new(data, address, hash)
}

/// Builds a new test blob for direct sequencer registration.
pub fn new_test_blob_for_direct_registration(
    tx: RawTx,
    address: &[u8],
    hash: [u8; 32],
) -> <MockDaSpec as DaSpec>::BlobTransaction {
    let batch = tx;
    let address = MockAddress::try_from(address).unwrap();
    let data = borsh::to_vec(&batch).unwrap();
    MockBlob::new(data, address, hash)
}

/// Checks if the given [`BatchReceipt`] contains any events.
pub fn has_tx_events<Da: DaSpec>(apply_blob_outcome: &BatchReceipt<Da>) -> bool {
    let events = apply_blob_outcome
        .tx_receipts
        .iter()
        .flat_map(|receipts| receipts.events.iter());

    events.peekable().peek().is_some()
}
