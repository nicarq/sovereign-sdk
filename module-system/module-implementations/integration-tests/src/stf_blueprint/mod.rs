use anyhow::bail;
use sov_mock_da::MockDaSpec;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::runtime::capabilities::RuntimeAuthorization;
use sov_modules_api::{Context, DaSpec, KernelWorkingSet, Spec, StateCheckpoint};
use sov_test_utils::runtime::TestRuntime;
use sov_test_utils::value_setter_data::ValueSetterMessages;
use sov_test_utils::{new_test_blob_from_batch, MessageGenerator};

use crate::helpers::{
    AttesterIncentivesParams, BankParams, Da, SequencerParams, TestRollup, DEFAULT_STAKE_AMOUNT, S,
};

impl TestRollup {
    // Check the current kernel height and that the context are correctly built
    pub(crate) fn check_kernel_and_context_updates(
        &mut self,
        expected_height: u64,
        value_setter_messages: &ValueSetterMessages<S>,
        seq_da_addr: <Da as DaSpec>::Address,
        seq_rollup_addr: <S as Spec>::Address,
    ) -> anyhow::Result<()> {
        let mut state_checkpoint = StateCheckpoint::new(self.storage());
        let kernel = self.kernel();
        let kernel_working_set = KernelWorkingSet::from_kernel(kernel, &mut state_checkpoint);

        let height = kernel_working_set.current_slot();

        if height != expected_height {
            bail!("The kernel height is not equal to the expected height.");
        }

        let admin_pub_key = value_setter_messages.messages[0]
            .admin
            .to_address::<<S as Spec>::Address>();

        let contexts: Vec<Context<S>> = value_setter_messages
            .create_default_messages()
            .into_iter()
            .map(|m| {
                self.stf()
                    .runtime()
                    .resolve_context(
                        &m.to_tx::<TestRuntime<S, Da>>().into(),
                        &seq_da_addr,
                        height,
                        &mut state_checkpoint,
                    )
                    .unwrap()
            })
            .collect();

        for context in contexts {
            if context != Context::new(admin_pub_key, seq_rollup_addr, height) {
                bail!("The context was not correctly built.");
            }
        }

        Ok(())
    }
}

/// Tests that the execution values of the `StfBlueprint` are correctly set.
/// In particular, these tests check that the `Kernel` gets updated correctly and
/// that the `Context` can be correctly built out of the `Kernel`.

/// Builds and executes a simple rollup and checks that the kernel and the context updates correctly.
#[test]
fn test_stf_internal_updates() {
    // Build a STF blueprint with the module configurations
    let mut rollup = TestRollup::new();

    let value_setter_messages = ValueSetterMessages::prepopulated();
    let value_setter = value_setter_messages.create_default_raw_txs::<TestRuntime<S, MockDaSpec>>();

    let admin_pub_key = value_setter_messages.messages[0]
        .admin
        .to_address::<<S as Spec>::Address>();

    let seq_params = SequencerParams::default();
    let seq_rollup_addr = seq_params.rollup_address;
    let seq_da_addr = seq_params.da_address;
    let bank_params = BankParams::with_addresses_and_balances(vec![
        (seq_params.rollup_address, DEFAULT_STAKE_AMOUNT),
        (admin_pub_key, DEFAULT_STAKE_AMOUNT),
    ]);
    let attester_params = AttesterIncentivesParams::default();

    // Genesis
    let init_root_hash = rollup.genesis(admin_pub_key, seq_params, bank_params, attester_params);

    assert!(rollup
        .check_kernel_and_context_updates(0, &value_setter_messages, seq_da_addr, seq_rollup_addr,)
        .is_ok());

    let blob = new_test_blob_from_batch(
        BatchWithId {
            txs: value_setter,
            id: [0; 32],
        },
        seq_da_addr.as_ref(),
        [0; 32],
    );

    let exec_simulation =
        rollup.execution_simulation(5, init_root_hash, vec![blob.clone()], 0, None);

    assert_eq!(exec_simulation.len(), 5, "The execution simulation failed");

    assert!(rollup
        .check_kernel_and_context_updates(5, &value_setter_messages, seq_da_addr, seq_rollup_addr,)
        .is_ok());
}
