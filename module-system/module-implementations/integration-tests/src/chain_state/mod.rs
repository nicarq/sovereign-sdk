use sov_chain_state::{ChainState, StateTransition, TransitionInProgress};
use sov_mock_da::{MockDaSpec, MockHash, MockValidityCond};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::{GasArray, GasPrice, KernelWorkingSet, StateCheckpoint, WorkingSet};
use sov_modules_stf_blueprint::SequencerOutcome;
use sov_test_utils::runtime::TestRuntime;
use sov_test_utils::value_setter_data::ValueSetterMessages;
use sov_test_utils::{has_tx_events, new_test_blob_from_batch, MessageGenerator, TestHasher};

use crate::helpers::{
    AttesterIncentivesParams, BankParams, SequencerParams, TestKernel, TestRollup,
    GAS_TX_FIXED_COST, INITIAL_GAS_PRICE,
};

type S = sov_test_utils::TestSpec;

const INITIAL_USER_BALANCE: u64 = 10000;

/// This test generates a new mock rollup having a simple value setter module
/// with an associated chain state, and checks that the height, the genesis hash
/// and the state transitions are correctly stored and updated.
#[test]
fn test_simple_value_setter_with_chain_state() {
    // Build a STF blueprint with the module configurations
    let mut rollup = TestRollup::new();

    let value_setter_messages = ValueSetterMessages::prepopulated();
    let value_setter = value_setter_messages.create_raw_txs::<TestRuntime<S, MockDaSpec>>();
    let num_value_setter_txs = value_setter.len();

    // We need to multiply each component of the gas used by 2 because there are 2 messages
    let gas_used_per_slot = GAS_TX_FIXED_COST.map(|g| num_value_setter_txs as u64 * g);

    let admin_pub_key = value_setter_messages.messages[0]
        .admin
        .to_address::<TestHasher, _>();
    let test_kernel = TestKernel::<S, MockDaSpec>::default();

    let seq_params = SequencerParams::default();
    let seq_da_addr = seq_params.da_address;
    let bank_params = BankParams::with_addresses_and_balances(vec![
        (admin_pub_key, INITIAL_USER_BALANCE),
        (seq_params.rollup_address, INITIAL_USER_BALANCE),
    ]);
    let attester_params = AttesterIncentivesParams::default();

    // Genesis
    let init_root_hash = rollup.genesis(admin_pub_key, seq_params, bank_params, attester_params);

    let blob = new_test_blob_from_batch(
        BatchWithId {
            txs: value_setter,
            id: [0; 32],
        },
        seq_da_addr.as_ref(),
        [2; 32],
    );

    {
        let mut init_working_set = StateCheckpoint::<S>::new(rollup.storage());

        // Computes the initial kernel working set
        let kernel_working_set = KernelWorkingSet::uninitialized(&mut init_working_set);

        let new_height_storage = {
            // Check the slot number before `apply_slot`
            kernel_working_set.current_slot()
        };

        assert_eq!(new_height_storage, 0, "The initial height was not computed");
    }

    let exec_simulation =
        rollup.execution_simulation(1, init_root_hash, vec![blob.clone()], 0, None);
    let first_root = exec_simulation[0].state_root;

    {
        assert_eq!(exec_simulation.len(), 1, "The execution simulation failed");

        let batch_receipts = exec_simulation[0].batch_receipts.clone();
        assert_eq!(1, batch_receipts.len());

        let apply_blob_outcome = batch_receipts[0].clone();
        assert_eq!(
            SequencerOutcome::Rewarded(0),
            apply_blob_outcome.inner,
            "Sequencer execution should have succeeded but failed "
        );

        // Computes the new working set after slot application
        let mut state_checkpoint = StateCheckpoint::new(rollup.storage());

        let chain_state_ref: &ChainState<S, MockDaSpec> = test_kernel.chain_state();

        // Check that the root hash has been stored correctly
        let stored_root = chain_state_ref
            .get_genesis_hash(&mut state_checkpoint)
            .unwrap();

        assert_eq!(stored_root, init_root_hash, "Root hashes don't match");

        // Check the slot number
        let mut kernel_working_set =
            KernelWorkingSet::from_kernel(&test_kernel, &mut state_checkpoint);
        let new_height_storage = kernel_working_set.current_slot();

        assert_eq!(new_height_storage, 1, "The new height did not update");

        // Check the tx in progress
        let new_tx_in_progress: TransitionInProgress<S, MockDaSpec> = chain_state_ref
            .get_in_progress_transition(&mut kernel_working_set)
            .unwrap();

        assert_eq!(
            new_tx_in_progress,
            TransitionInProgress::<S, MockDaSpec>::new(
                MockHash::from([10; 32]),
                MockValidityCond::default(),
                GasPrice::from_slice(&INITIAL_GAS_PRICE),
                GasArray::from_slice(&gas_used_per_slot)
            ),
            "The new transition has not been correctly stored"
        );

        assert!(has_tx_events(&apply_blob_outcome),);
    }

    let exec_simulation = rollup.execution_simulation(1, first_root, vec![blob], 1, None);

    #[cfg(test)]
    {
        assert_eq!(exec_simulation.len(), 1, "The execution simulation failed");

        let batch_receipts = exec_simulation[0].batch_receipts.clone();
        assert_eq!(1, batch_receipts.len());
        let apply_blob_outcome = batch_receipts[0].clone();
        assert_eq!(
            SequencerOutcome::Rewarded(0),
            apply_blob_outcome.inner,
            "Sequencer execution should have succeeded but failed "
        );

        // Computes the new working set after slot application
        let mut working_set = WorkingSet::new(rollup.storage());

        let chain_state_ref: &ChainState<S, MockDaSpec> = test_kernel.chain_state();

        // Check that the root hash has been stored correctly
        let stored_root = chain_state_ref.get_genesis_hash(&mut working_set).unwrap();

        assert_eq!(stored_root, init_root_hash, "Root hashes don't match");

        // Check the slot number
        let mut state_checkpoint = working_set.checkpoint().0;
        let mut kernel_working_set =
            KernelWorkingSet::from_kernel(&test_kernel, &mut state_checkpoint);
        let new_height_storage = chain_state_ref.true_slot_number(&mut kernel_working_set);
        assert_eq!(new_height_storage, 2, "The new height did not update");

        // Check the tx in progress
        let new_tx_in_progress: TransitionInProgress<S, MockDaSpec> = chain_state_ref
            .get_in_progress_transition(&mut kernel_working_set)
            .unwrap();

        assert_eq!(
            new_tx_in_progress,
            TransitionInProgress::<S, MockDaSpec>::new(
                [20; 32].into(),
                MockValidityCond::default(),
                GasPrice::from_slice(&INITIAL_GAS_PRICE),
                GasArray::from_slice(&gas_used_per_slot)
            ),
            "The new transition has not been correctly stored"
        );

        let last_tx_stored: StateTransition<S, MockDaSpec> = chain_state_ref
            .get_historical_transitions(1, &mut state_checkpoint)
            .unwrap();

        let expected_tx_stored: StateTransition<S, MockDaSpec> =
            StateTransition::<S, MockDaSpec>::new(
                [10; 32].into(),
                first_root,
                MockValidityCond::default(),
                GasPrice::from_slice(&INITIAL_GAS_PRICE),
                GasArray::from_slice(&gas_used_per_slot),
            );

        assert_eq!(last_tx_stored, expected_tx_stored);
    }
}
