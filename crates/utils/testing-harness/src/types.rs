use demo_stf::authentication::ModAuth;
use derive_more::Constructor;
use sov_celestia_adapter::verifier::CelestiaSpec;
use sov_mock_zkvm::MockZkVerifier;
use sov_modules_api::default_spec::DefaultSpec;
use sov_modules_api::{Module, Spec};
use sov_risc0_adapter::Risc0Verifier;
use sov_rollup_interface::execution_mode::Native;

pub type ThisSpec = DefaultSpec<Risc0Verifier, MockZkVerifier, Native>;
/// Shortcut for module native authorization.
pub type Auth = ModAuth<ThisSpec, CelestiaSpec>;

/// Combination of a module specific call message with expected sender.
#[derive(Debug, Constructor)]
pub struct PreparedCallMessage<S: Spec, M: Module<Spec = S>> {
    pub call_message: M::CallMessage,
    pub from: S::Address,
    pub max_fee: u64,
}
