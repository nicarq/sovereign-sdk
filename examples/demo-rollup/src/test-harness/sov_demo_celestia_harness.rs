use demo_stf::authentication::ModAuth;
use sov_celestia_adapter::verifier::CelestiaSpec;
use sov_celestia_adapter::CelestiaService;
use sov_demo_rollup::initialize_logging;
use sov_mock_zkvm::MockZkVerifier;
use sov_modules_api::default_spec::DefaultSpec;
use sov_risc0_adapter::Risc0Verifier;
use sov_rollup_interface::execution_mode::Native;

pub(crate) type ThisSpec = DefaultSpec<Risc0Verifier, MockZkVerifier, Native>;
pub(crate) type ThisAuth = ModAuth<ThisSpec, CelestiaSpec>;
pub(crate) type ThisDaService = CelestiaService;
pub(crate) type ThisRuntime = demo_stf::runtime::Runtime<ThisSpec, CelestiaSpec>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_logging();
    sov_test_harness::start::<ThisSpec, ThisDaService, ThisAuth, ThisRuntime>().await
}
