// TODO: Rename this file to change the name of this method from METHOD_NAME

#![no_main]

use const_rollup_config::ROLLUP_BATCH_NAMESPACE_RAW;
use demo_stf::runtime::Runtime;
use demo_stf::StfVerifier;
use sov_celestia_adapter::types::Namespace;
use sov_celestia_adapter::verifier::CelestiaVerifier;
use sov_modules_api::default_spec::ZkDefaultSpec;
use sov_modules_stf_blueprint::{kernels::basic::BasicKernel, StfBlueprint};
use sov_risc0_adapter::guest::Risc0Guest;
use sov_risc0_adapter::Risc0Verifier;
use sov_state::ZkStorage;

// The rollup stores its data in the namespace b"sov-test" on Celestia
const ROLLUP_NAMESPACE: Namespace = Namespace::const_v0(ROLLUP_BATCH_NAMESPACE_RAW);

risc0_zkvm::guest::entry!(main);

pub fn main() {
    let guest = Risc0Guest::new();
    let storage = ZkStorage::new();
    let stf: StfBlueprint<
        ZkDefaultSpec<Risc0Verifier>,
        _,
        Risc0Verifier,
        Runtime<_, _>,
        BasicKernel<_, _>,
    > = StfBlueprint::new();

    let stf_verifier = StfVerifier::<_, Risc0Guest, _, _, _>::new(
        stf,
        CelestiaVerifier {
            rollup_namespace: ROLLUP_NAMESPACE,
        },
    );
    stf_verifier
        .run_block(guest, storage)
        .expect("Prover must be honest");
}
