use sov_chain_state::{
    BlockGasInfo, ChainState, ChainStateConfig, StateTransition, TransitionInProgress,
};
use sov_mock_da::{MockBlock, MockBlockHeader, MockDaSpec, MockValidityCond};
use sov_mock_zkvm::MockCodeCommitment;
use sov_modules_api::da::Time;
use sov_modules_api::{Gas, KernelModule, KernelWorkingSet, Spec};
use sov_modules_core::runtime::capabilities::mocks::MockKernel;
use sov_modules_core::StateCheckpoint;
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::{DefaultStorageSpec, ProverStorage, Storage, StorageRoot};
use sov_test_utils::TestSpec;

const INITIAL_BASE_FEE_PER_GAS: [u64; 2] = [1, 1];
const NUM_ROUNDS: u8 = 4;

/// Helper function that initializes the integration test. It creates and configures a simple chain state with [`INITIAL_BASE_FEE_PER_GAS`] base fee per gas.
/// Then it runs and commits the genesis state and returns a [`ChainState`] object,  the `genesis_root` (as a [`StorageRoot`]) and the `storage` (which is a [`ProverStorage`]).
fn init_test() -> (
    ChainState<TestSpec, MockDaSpec>,
    StorageRoot<DefaultStorageSpec>,
    ProverStorage<DefaultStorageSpec>,
) {
    // The initial height can be any value.
    // Initialize the module.
    let tmpdir = tempfile::tempdir().unwrap();

    let storage = new_orphan_storage(tmpdir.path()).unwrap();
    let mut state_checkpoint = StateCheckpoint::new(storage.clone());

    let chain_state = ChainState::<TestSpec, MockDaSpec>::default();
    let config = ChainStateConfig {
        current_time: Default::default(),
        initial_base_fee_per_gas: INITIAL_BASE_FEE_PER_GAS.into(),
        genesis_da_height: 0,
        inner_code_commitment: MockCodeCommitment::default(),
        outer_code_commitment: MockCodeCommitment::default(),
    };

    // Genesis, initialize and then commit the state
    chain_state
        .genesis_unchecked(
            &config,
            &mut KernelWorkingSet::uninitialized(&mut state_checkpoint),
        )
        .unwrap();
    let (reads_writes, _, witness) = state_checkpoint.freeze();
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

    // Check that the genesis state variables are correctly initialized.
    assert_eq!(initial_height, 0, "The initial height was not computed");
    assert_eq!(
        chain_state.get_time(&mut working_set),
        Default::default(),
        "The time was not initialized to default value"
    );

    (chain_state, genesis_root, storage)
}

/// Simulates one round of [`ChainState`] execution.
/// Simply calls the [`ChainState::begin_slot_hook`] and [`ChainState::end_slot_hook`] hooks successively
/// for a [`MockBlock`] whose header has a height of `round_num`, a creation time set to [`Time::now`],
/// a previous hash of `[round_num - 1; 32]`, and a current hash of `[round_num; 32]`.
/// Returns the creation time embedded in the `slot_data` header.
fn simulate_chain_state_execution(
    round_num: u8,
    validity_cond: MockValidityCond,
    pre_state_root: &StorageRoot<DefaultStorageSpec>,
    gas_used: &<TestSpec as Spec>::Gas,
    chain_state: &ChainState<TestSpec, MockDaSpec>,
    kernel_working_set: &mut KernelWorkingSet<TestSpec>,
) -> Time {
    // Sanity test: the round number matches the next height of the chain and so cannot be zero.
    assert!(round_num > 0);

    let slot_data = MockBlock {
        header: MockBlockHeader {
            prev_hash: [round_num - 1; 32].into(),
            hash: [round_num; 32].into(),
            height: round_num as u64,
            time: Time::now(),
        },
        validity_cond,
        batch_blobs: Default::default(),
        proof_blobs: Default::default(),
    };

    chain_state.begin_slot_hook(
        &slot_data.header,
        &slot_data.validity_cond,
        pre_state_root,
        kernel_working_set,
    );
    chain_state.end_slot_hook(gas_used, kernel_working_set);

    slot_data.header.time
}

/// Checks that the [`ChainState`] time state variable is correctly updated after each round of execution.
fn check_time_updates(
    time: Time,
    chain_state: &ChainState<TestSpec, MockDaSpec>,
    kernel_working_set: &mut KernelWorkingSet<TestSpec>,
) {
    assert_ne!(
        chain_state.get_time(kernel_working_set),
        Default::default(),
        "The time must be updated"
    );

    assert_eq!(
        chain_state.get_time(kernel_working_set),
        time,
        "The time was not updated in the hook"
    );
}

/// Checks that the [`ChainState`] state transitions are correctly stored after each round of execution.
fn check_transitions_stored(
    round_num: u8,
    pre_state_root: StorageRoot<DefaultStorageSpec>,
    gas_used: &<TestSpec as Spec>::Gas,
    chain_state: &ChainState<TestSpec, MockDaSpec>,
    kernel_working_set: &mut KernelWorkingSet<TestSpec>,
) {
    // Sanity test: the round number matches the next height of the chain and so cannot be zero.
    assert!(round_num > 0);

    // Check that the new state transition is being stored
    let new_tx_in_progress: TransitionInProgress<TestSpec, MockDaSpec> = chain_state
        .get_in_progress_transition(kernel_working_set)
        .unwrap();

    let mut block_gas_info = BlockGasInfo::new(
        ChainState::<TestSpec, MockDaSpec>::initial_gas_limit(),
        INITIAL_BASE_FEE_PER_GAS.into(),
    );

    block_gas_info.update_gas_used(gas_used.clone());

    assert_eq!(
        new_tx_in_progress,
        TransitionInProgress::<TestSpec, MockDaSpec>::new(
            [round_num; 32].into(),
            MockValidityCond { is_valid: true },
            block_gas_info
        ),
        "The new transition has not been correctly stored"
    );

    if round_num == 1 {
        // Check that the genesis root hash has been stored correctly
        let stored_root = chain_state.get_genesis_hash(kernel_working_set).unwrap();

        assert_eq!(stored_root, pre_state_root, "Genesis hashes don't match");
    } else {
        // Check that the last state transition has been stored in the `historical_transitions` map.
        let last_tx_stored: StateTransition<TestSpec, MockDaSpec> = chain_state
            .get_historical_transitions((round_num - 1) as u64, kernel_working_set.inner)
            .unwrap();

        let mut block_gas_info = BlockGasInfo::new(
            ChainState::<TestSpec, MockDaSpec>::initial_gas_limit(),
            INITIAL_BASE_FEE_PER_GAS.into(),
        );

        block_gas_info.update_gas_used(gas_used.clone());

        let expected_tx_stored: StateTransition<TestSpec, MockDaSpec> = StateTransition::new(
            [round_num - 1; 32].into(),
            pre_state_root,
            MockValidityCond { is_valid: true },
            block_gas_info,
        );

        assert_eq!(
            last_tx_stored, expected_tx_stored,
            "The stored transition data must match"
        );
    }
}

/// Checks that the [`ChainState`] has correclty been updated after each round of execution.
fn post_simulation_state_checks(
    round_num: u8,
    pre_state_root: StorageRoot<DefaultStorageSpec>,
    time: Time,
    gas_used: &<TestSpec as Spec>::Gas,
    chain_state: &ChainState<TestSpec, MockDaSpec>,
    kernel_working_set: &mut KernelWorkingSet<TestSpec>,
) {
    // Check that the slot number has been updated
    let new_height_storage = chain_state.true_slot_number(kernel_working_set);

    assert_eq!(
        new_height_storage, round_num as u64,
        "The new height did not update"
    );

    // Check that the time state variable has been updated
    check_time_updates(time, chain_state, kernel_working_set);

    // Check that the state transitions have been correctly stored
    check_transitions_stored(
        round_num,
        pre_state_root,
        gas_used,
        chain_state,
        kernel_working_set,
    );
}

/// Builds a new [`KernelWorkingSet`] that has `round_num` as the true and visible slot number.
fn build_kernel_working_set(
    round_num: u8,
    state_checkpoint: &mut StateCheckpoint<TestSpec>,
) -> KernelWorkingSet<TestSpec> {
    let mock_kernel: MockKernel<TestSpec, MockDaSpec> =
        MockKernel::new((round_num - 1) as u64, (round_num - 1) as u64);
    KernelWorkingSet::from_kernel(&mock_kernel, state_checkpoint)
}

/// Simulates the execution of the chain state from genesis to `round_num` slots.
/// For each round, this method calls the [`ChainState::begin_slot_hook`] and [`ChainState::end_slot_hook`] hooks successively
/// for a [`MockBlock`] whose header has a height of `round_num`, a creation time set to [`Time::now`],
/// a previous hash of `[round_num - 1; 32]`, and a current hash of `[round_num; 32]`.
/// Then it checks that the [`ChainState`] state variables have been correctly updated and commits the writes to the storage.
fn simulate_chain_state_execution_n_rounds(
    n_rounds: u8,
    genesis_root: StorageRoot<DefaultStorageSpec>,
    gas_used: &<TestSpec as Spec>::Gas,
    chain_state: &ChainState<TestSpec, MockDaSpec>,
    storage: ProverStorage<DefaultStorageSpec>,
) {
    assert!(n_rounds > 0);

    let mut pre_state_root = genesis_root;

    for round_num in 1..n_rounds {
        let mut state_checkpoint = StateCheckpoint::new(storage.clone());
        let mut kernel_working_set = build_kernel_working_set(round_num, &mut state_checkpoint);

        let time = simulate_chain_state_execution(
            round_num,
            MockValidityCond { is_valid: true },
            &pre_state_root,
            gas_used,
            chain_state,
            &mut kernel_working_set,
        );

        post_simulation_state_checks(
            round_num,
            pre_state_root,
            time,
            gas_used,
            chain_state,
            &mut kernel_working_set,
        );

        // We now commit the new state (which updates the root hash)
        let (reads_writes, _, witness) = state_checkpoint.freeze();

        pre_state_root = storage.validate_and_commit(reads_writes, &witness).unwrap();
    }
}

/// This test simulates the execution of the chain state for genesis and one slot after. It checks that the
/// chain state updates its state properly with the invocation of the [`ChainState::begin_slot_hook`] and [`ChainState::end_slot_hook`] hooks.  
///
/// For more complete integration tests, feel free to have a look at the integration tests folder.
#[test]
fn test_simple_chain_state_one_round() {
    // Initialize the test: create and configure a simple chain state with [`INITIAL_BASE_FEE_PER_GAS`] base fee per gas.
    // Then run and commit the genesis state and returns the `storage` and `genesis_root`.
    let (chain_state, genesis_root, storage) = init_test();

    // Then simulate a transaction execution: call the begin_slot hook on a mock slot_data.
    simulate_chain_state_execution_n_rounds(
        1,
        genesis_root,
        &<TestSpec as Spec>::Gas::zero(),
        &chain_state,
        storage,
    );
}

/// This test simulates the execution of the chain state for genesis and [`NUM_ROUNDS`] slots after. It checks that the
/// chain state updates its state properly with the invocation of the [`ChainState::begin_slot_hook`] and [`ChainState::end_slot_hook`] hooks.  
///
/// For more complete integration tests, feel free to have a look at the integration tests folder.
#[test]
fn test_simple_chain_state() {
    // Initialize the test: create and configure a simple chain state with [`INITIAL_BASE_FEE_PER_GAS`] base fee per gas.
    // Then run and commit the genesis state and returns the `storage` and `genesis_root`.
    let (chain_state, genesis_root, storage) = init_test();

    // Then simulate a transaction execution: call the begin_slot hook on a mock slot_data.
    simulate_chain_state_execution_n_rounds(
        NUM_ROUNDS,
        genesis_root,
        &<TestSpec as Spec>::Gas::zero(),
        &chain_state,
        storage,
    );
}
