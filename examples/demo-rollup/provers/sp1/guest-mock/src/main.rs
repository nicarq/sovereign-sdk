#![no_main]

sp1_zkvm::entrypoint!(main);

use demo_stf::runtime::Runtime;
use demo_stf::StfVerifier;
use sov_kernels::basic::BasicKernel;
use sov_mock_da::MockDaVerifier;
pub use sov_mock_zkvm::{MockZkGuest, MockZkVerifier};
use sov_modules_api::default_spec::DefaultSpec;
use sov_modules_api::execution_mode::Zk;
use sov_modules_stf_blueprint::StfBlueprint;
use sov_sp1_adapter::guest::SP1Guest;
use sov_sp1_adapter::SP1Verifier;
use sov_state::ZkStorage;

#[cfg_attr(feature = "bench", sov_cycle_utils::macros::cycle_tracker)]
pub fn main() {
    let guest = SP1Guest::new();
    let storage = ZkStorage::new();

    let stf: StfBlueprint<
        DefaultSpec<SP1Verifier, MockZkVerifier, Zk>,
        _,
        Runtime<_, _>,
        BasicKernel<_, _>,
    > = StfBlueprint::new();

    let stf_verifier = StfVerifier::<_, _, _, _, _, MockZkGuest>::new(stf, MockDaVerifier {});

    stf_verifier
        .run_block(guest, storage)
        .expect("Prover must be honest");
}
