use sov_celestia_adapter::verifier::CelestiaSpec;
use sov_celestia_adapter::CelestiaService;
use sov_demo_rollup::initialize_logging;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::default_spec::DefaultSpec;
use sov_risc0_adapter::Risc0;
use sov_rollup_interface::execution_mode::Native;
use sov_rollup_interface::reexports::anyhow;

pub(crate) type ThisSpec = DefaultSpec<CelestiaSpec, Risc0, MockZkvm, Native>;
pub(crate) type ThisDaService = CelestiaService;
pub(crate) type ThisRuntime = demo_stf::runtime::Runtime<ThisSpec>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_logging();
    sov_test_harness::start::<ThisSpec, ThisDaService, ThisRuntime>().await
}
