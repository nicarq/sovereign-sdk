use sov_hyperlane_integration::test_recipient::TestRecipient;
use sov_hyperlane_integration::{HyperlaneAddress, Mailbox as RawMailbox, MerkleTreeHook};
use sov_test_utils::generate_runtime;

pub type Mailbox<S> = RawMailbox<S, TestRecipient<S>>;

generate_runtime! {
    name: TestRuntime,
    modules: [mailbox: Mailbox<S>, test_recipient: TestRecipient<S>, merkle_tree_hook: MerkleTreeHook<S>],
    operating_mode: sov_modules_api::runtime::OperatingMode::Zk,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::zk::config::MinimalZkGenesisConfig<S>,
    runtime_trait_impl_bounds: [S::Address: HyperlaneAddress],
    kernel_type: sov_test_utils::runtime::SoftConfirmationsKernel<'a, S>,
    auth_type: sov_modules_api::capabilities::RollupAuthenticator<S, TestRuntime<S>>,
    auth_call_wrapper: |call| call,
}
