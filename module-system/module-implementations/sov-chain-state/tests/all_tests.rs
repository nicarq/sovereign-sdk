use sov_chain_state::{ChainState, ChainStateConfig, StateTransitionId, TransitionInProgress};
use sov_mock_da::{MockBlock, MockBlockHeader, MockDaSpec, MockValidityCond};
use sov_modules_api::da::{BlockHeaderTrait, Time};
use sov_modules_api::{Gas, GasPrice, GasUnit, KernelModule, KernelWorkingSet, Spec};
use sov_modules_core::runtime::capabilities::mocks::MockKernel;
use sov_modules_core::StateCheckpoint;
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::Storage;
use sov_test_utils::TestSpec;

/// This simply tests that the chain_state reacts properly with the invocation of the `begin_slot`
/// hook. For more complete integration tests, feel free to have a look at the integration tests folder.
#[test]
fn test_simple_chain_state() {
    // The initial height can be any value.
    // Initialize the module.
    let tmpdir = tempfile::tempdir().unwrap();

    let storage = new_orphan_storage(tmpdir.path()).unwrap();
    let mut state_checkpoint = StateCheckpoint::new(storage.clone());

    let chain_state = ChainState::<TestSpec, MockDaSpec>::default();
    let initial_gas_price: GasPrice<2> = [2000, 2000].into();
    let gas_price_maximum_elasticity = 1;
    let minimum_gas_price: GasPrice<2> = [1, 1].into();
    let config = ChainStateConfig {
        current_time: Default::default(),
        gas_price_blocks_depth: 10,
        gas_price_maximum_elasticity,
        initial_gas_price: initial_gas_price.clone(),
        minimum_gas_price: minimum_gas_price.clone(),
    };

    // Genesis, initialize and then commit the state
    chain_state
        .genesis_unchecked(
            &config,
            &mut KernelWorkingSet::uninitialized(&mut state_checkpoint),
        )
        .unwrap();
    let (reads_writes, witness) = state_checkpoint.freeze();
    let genesis_root = storage.validate_and_commit(reads_writes, &witness).unwrap();

    // Computes the initial, post genesis, working set
    let mut base_checkpoint = StateCheckpoint::new(storage.clone());

    // Check the slot number before any changes to the state.
    let mock_kernel: MockKernel<TestSpec, MockDaSpec> = MockKernel::new(0, 0);
    let initial_height = chain_state.true_slot_number(&mut KernelWorkingSet::from_kernel(
        &mock_kernel,
        &mut base_checkpoint,
    ));
    let mut working_set = KernelWorkingSet::from_kernel(&mock_kernel, &mut base_checkpoint);

    assert_eq!(initial_height, 0, "The initial height was not computed");
    assert_eq!(
        chain_state.get_time(&mut working_set),
        Default::default(),
        "The time was not initialized to default value"
    );

    // Then simulate a transaction execution: call the begin_slot hook on a mock slot_data.
    let slot_data = MockBlock {
        header: MockBlockHeader {
            prev_hash: [0; 32].into(),
            hash: [1; 32].into(),
            height: 1,
            time: Time::now(),
        },
        validity_cond: MockValidityCond { is_valid: true },
        blobs: Default::default(),
    };

    chain_state.begin_slot_hook(
        &slot_data.header,
        &slot_data.validity_cond,
        &genesis_root,
        &mut working_set,
    );
    chain_state.end_slot_hook(&Gas::zero(), &mut working_set);

    // Check that the root hash has been stored correctly
    let stored_root = chain_state.get_genesis_hash(working_set.inner).unwrap();

    assert_eq!(stored_root, genesis_root, "Genesis hashes don't match");
    assert_eq!(
        chain_state.get_time(&mut working_set),
        slot_data.header.time(),
        "The time was not updated in the hook"
    );

    // Check that the slot number has been updated
    let new_height_storage = chain_state.true_slot_number(&mut working_set);

    assert_eq!(new_height_storage, 1, "The new height did not update");

    // Update the kernel
    let mock_kernel: MockKernel<TestSpec, MockDaSpec> =
        MockKernel::new(new_height_storage, new_height_storage);

    // Check that the new state transition is being stored
    let new_tx_in_progress: TransitionInProgress<TestSpec, MockDaSpec> = chain_state
        .get_in_progress_transition(&mut working_set)
        .unwrap();

    let expected_gas_price = initial_gas_price.clone();
    let expected_gas_used = [0, 0].into();

    assert_eq!(
        new_tx_in_progress,
        TransitionInProgress::<TestSpec, MockDaSpec>::new(
            [1; 32].into(),
            MockValidityCond { is_valid: true },
            expected_gas_price,
            expected_gas_used,
        ),
        "The new transition has not been correctly stored"
    );

    // We now commit the new state (which updates the root hash)
    let (reads_writes, witness) = base_checkpoint.freeze();
    let new_root_hash = storage.validate_and_commit(reads_writes, &witness).unwrap();

    // Computes the new working set
    let mut base_working_set = StateCheckpoint::new(storage);
    let mut working_set = KernelWorkingSet::from_kernel(&mock_kernel, &mut base_working_set);

    // And we simulate a new slot application by calling the `begin_slot` hook.
    let new_slot_data = MockBlock {
        header: MockBlockHeader {
            prev_hash: [1; 32].into(),
            hash: [2; 32].into(),
            height: 2,
            time: Time::now(),
        },
        validity_cond: MockValidityCond { is_valid: false },
        blobs: Default::default(),
    };

    chain_state.begin_slot_hook(
        &new_slot_data.header,
        &new_slot_data.validity_cond,
        &new_root_hash,
        &mut working_set,
    );
    chain_state.end_slot_hook(&Gas::zero(), &mut working_set);

    // Check that the slot number has been updated correctly
    let new_height_storage = chain_state.true_slot_number(&mut working_set);
    assert_eq!(new_height_storage, 2, "The new height did not update");
    assert_eq!(
        chain_state.get_time(&mut working_set),
        new_slot_data.header.time(),
        "The time was not updated in the hook"
    );

    // Update the kernel
    let _mock_kernel: MockKernel<TestSpec, MockDaSpec> =
        MockKernel::new(new_height_storage, new_height_storage);

    // Check the transition in progress
    let new_tx_in_progress: TransitionInProgress<TestSpec, MockDaSpec> = chain_state
        .get_in_progress_transition(&mut working_set)
        .unwrap();

    // no gas was consumed
    let expected_gas_price = initial_gas_price.clone();
    let expected_gas_used: GasUnit<2> = [0, 0].into();

    assert_eq!(
        new_tx_in_progress,
        TransitionInProgress::<TestSpec, MockDaSpec>::new(
            [2; 32].into(),
            MockValidityCond { is_valid: false },
            expected_gas_price.clone(),
            expected_gas_used.clone(),
        ),
        "The new transition has not been correctly stored"
    );

    // Check the transition stored
    let last_tx_stored: StateTransitionId<TestSpec, MockDaSpec> = chain_state
        .get_historical_transitions(1, working_set.inner)
        .unwrap();
    let expected_tx_stored: StateTransitionId<TestSpec, MockDaSpec> = StateTransitionId::new(
        [1; 32].into(),
        new_root_hash,
        MockValidityCond { is_valid: true },
        expected_gas_price.clone(),
        expected_gas_used.clone(),
    );

    assert_eq!(
        borsh::to_vec(&last_tx_stored).unwrap(),
        borsh::to_vec(&expected_tx_stored).unwrap(),
        "The stored transition data must match"
    );

    assert_ne!(
        chain_state.get_time(&mut working_set),
        Default::default(),
        "The time must be updated"
    );

    // override the gas used of the current block so the elasticity of the price can be tested for
    // the next block
    let gas_used: GasUnit<2> = [10000, 10000].into();
    // the expected target is the average consumption of the previous two blocks, that are [0, 0]
    // and [10000, 10000]
    let expected_gas_target = [5000, 5000].into();
    let mut transition = chain_state
        .get_in_progress_transition(&mut working_set)
        .expect("the transition was performed");
    transition.override_gas_used(gas_used.clone());
    chain_state.override_in_progress_transition(transition, &mut working_set);

    let new_slot_data = MockBlock {
        header: MockBlockHeader {
            prev_hash: [2; 32].into(),
            hash: [3; 32].into(),
            height: 3,
            time: Time::now(),
        },
        validity_cond: MockValidityCond { is_valid: false },
        blobs: Default::default(),
    };

    chain_state.begin_slot_hook(
        &new_slot_data.header,
        &new_slot_data.validity_cond,
        &new_root_hash,
        &mut working_set,
    );
    chain_state.end_slot_hook(&Gas::zero(), &mut working_set);

    // Check that the slot number has been updated correctly
    let new_height_storage = chain_state.true_slot_number(&mut working_set);
    assert_eq!(new_height_storage, 3, "The new height did not update");
    assert_eq!(
        chain_state.get_time(&mut working_set),
        new_slot_data.header.time(),
        "The time was not updated in the hook"
    );

    // Check the transition in progress
    let new_tx_in_progress: TransitionInProgress<TestSpec, MockDaSpec> = chain_state
        .get_in_progress_transition(&mut working_set)
        .unwrap();

    // gas price should sharply increase due to very high emulated demand
    let new_gas_price = new_tx_in_progress.gas_price().clone();
    assert!(new_gas_price > initial_gas_price);

    let expected_gas_price = <TestSpec as Spec>::Gas::elastic_price(
        gas_price_maximum_elasticity,
        &expected_gas_target,
        &gas_used,
        &initial_gas_price,
        &minimum_gas_price,
    );

    assert_eq!(new_gas_price, expected_gas_price);
}
