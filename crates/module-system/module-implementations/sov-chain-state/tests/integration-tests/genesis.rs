use sov_chain_state::ChainState;
use sov_test_utils::AsUser;
use sov_value_setter::ValueSetter;

use crate::{setup, MockDaSpec, S};

#[test]
fn chain_state_kernel_genesis() {
    let (_, mut runner) = setup();

    runner.query_kernel_state(|kernel| {
        assert_eq!(
            kernel.current_slot(),
            0,
            "The kernel should be initialized to zero"
        );

        assert_eq!(
            kernel.virtual_slot(),
            0,
            "The kernel virtual slot should be initialized to zero"
        );
    });
}

#[test]
fn test_chain_state_genesis_root() {
    let (admin, mut runner) = setup();

    let genesis_state_root = *runner.state_root();

    runner.execute(
        admin.create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10)),
        None,
    );

    runner.query_kernel_state(|kernel| {
        assert_eq!(
            ChainState::<S, MockDaSpec>::default()
                .get_genesis_hash(kernel)
                .unwrap(),
            Some(genesis_state_root),
            "The genesis hash should be set"
        );
    });
}
