#![no_main]

use demo_stf::runtime::Runtime;
use demo_stf::StfVerifier;
use sov_address::MultiAddressEvm;
use sov_mock_da::{MockDaSpec, MockDaVerifier};
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::configurable_spec::ConfigurableSpec;
use sov_modules_api::execution_mode::Zk;
use sov_modules_stf_blueprint::StfBlueprint;
use sov_ligetron_adapter::guest::LigetronGuest;
use sov_ligetron_adapter::Ligetron;
use sov_state::ZkStorage;

// IMPORTANT:
// This program mirrors the RISC0/SP1 guest logic for the Mock DA case, but targets Ligetron.
// It expects the Ligetron runtime to supply hints (as a single bincode blob) and will commit
// the bincode-encoded public journal. Real runs require the Ligetron toolchain to pass hints
// and capture the journal according to the sov_journal contract.

#[no_mangle]
pub extern "C" fn main() {
    let guest = LigetronGuest::new();
    let storage = ZkStorage::new();

    let stf: StfBlueprint<
        ConfigurableSpec<MockDaSpec, Ligetron, MockZkvm, MultiAddressEvm, Zk>,
        Runtime<_>,
    > = StfBlueprint::new();

    let stf_verifier = StfVerifier::<_, _, _, _, _>::new(stf, MockDaVerifier {});

    stf_verifier
        .run_block(guest, storage)
        .expect("Prover must be honest");
}
