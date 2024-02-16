use sov_chain_state::{ChainState, TransitionInProgress};
use sov_mock_da::{MockBlock, MockBlockHeader, MockDaSpec, MockHash, MockValidityCond};
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::default_context::DefaultContext;
use sov_modules_api::{Gas, GasArray, GasPrice, KernelWorkingSet, StateCheckpoint, WorkingSet};
use sov_modules_stf_blueprint::kernels::basic::BasicKernel;
use sov_modules_stf_blueprint::{SequencerOutcome, StfBlueprint};
use sov_prover_storage_manager::SimpleStorageManager;
use sov_rollup_interface::da::Time;
use sov_rollup_interface::stf::{SlotResult, StateTransitionFunction};
use sov_test_utils::value_setter_data::ValueSetterMessages;
use sov_test_utils::{has_tx_events, new_test_blob_from_batch, MessageGenerator};

use crate::chain_state::helpers::{create_chain_state_genesis_config, TestKernel, TestRuntime};
type C = DefaultContext;

/// This test generates a new mock rollup having a simple value setter module
/// with an associated chain state, and checks that the height, the genesis hash
/// and the state transitions are correctly stored and updated.
#[test]
fn test_simple_value_setter_with_chain_state() {
    // Build a STF blueprint with the module configurations

    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
    let storage = storage_manager.create_storage();

    let stf = StfBlueprint::<
        C,
        MockDaSpec,
        MockZkvm<MockValidityCond>,
        TestRuntime<C, MockDaSpec>,
        BasicKernel<C, MockDaSpec>,
    >::new();

    let value_setter_messages = ValueSetterMessages::default();
    let value_setter = value_setter_messages.create_raw_txs::<TestRuntime<C, MockDaSpec>>();

    let admin_pub_key = value_setter_messages.messages[0].admin.default_address();
    let test_kernel = TestKernel::<C, MockDaSpec>::default();

    const MOCK_SEQUENCER_DA_ADDRESS: [u8; 32] = [1_u8; 32];
    const INIT_BALANCE: u64 = 100000000;
    const SEQUENCER_STAKE_AMOUNT: u64 = 10000;
    const TOKEN_NAME: &str = "TEST_TOKEN";
    const SALT: u64 = 0;

    // Genesis
    let (init_root_hash, stf_change_set) = stf.init_chain(
        storage,
        create_chain_state_genesis_config::<C, MockDaSpec>(
            admin_pub_key,
            MOCK_SEQUENCER_DA_ADDRESS.into(),
            MOCK_SEQUENCER_DA_ADDRESS.into(),
            SEQUENCER_STAKE_AMOUNT,
            TOKEN_NAME.to_string(),
            SALT,
            INIT_BALANCE,
        ),
    );
    storage_manager.commit(stf_change_set);
    let storage = storage_manager.create_storage();

    let blob = new_test_blob_from_batch(
        BatchWithId {
            txs: value_setter,
            id: [0; 32],
        },
        &MOCK_SEQUENCER_DA_ADDRESS,
        [2; 32],
    );

    let slot_data: MockBlock = MockBlock {
        header: MockBlockHeader {
            prev_hash: [0; 32].into(),
            hash: [10; 32].into(),
            height: 0,
            time: Time::now(),
        },
        validity_cond: MockValidityCond::default(),
        blobs: vec![blob.clone()],
    };

    {
        let mut init_working_set = StateCheckpoint::<C>::new(storage.clone());

        // Computes the initial kernel working set
        let kernel_working_set = KernelWorkingSet::uninitialized(&mut init_working_set);

        let new_height_storage = {
            // Check the slot number before `apply_slot`
            kernel_working_set.current_slot()
        };

        assert_eq!(new_height_storage, 0, "The initial height was not computed");
    }

    let SlotResult {
        state_root: new_root_hash,
        change_set,
        batch_receipts,
        ..
    } = stf.apply_slot(
        &init_root_hash,
        storage,
        Default::default(),
        &slot_data.header,
        &slot_data.validity_cond,
        &mut [blob.clone()],
    );
    storage_manager.commit(change_set);

    {
        assert_eq!(1, batch_receipts.len());
        let apply_blob_outcome = batch_receipts[0].clone();
        assert_eq!(
            SequencerOutcome::Rewarded(0),
            apply_blob_outcome.inner,
            "Sequencer execution should have succeeded but failed "
        );

        // Computes the new working set after slot application
        let storage = storage_manager.create_storage();
        let mut state_checkpoint = StateCheckpoint::new(storage.clone());

        let chain_state_ref: &ChainState<C, MockDaSpec> = test_kernel.chain_state();

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
        let new_tx_in_progress: TransitionInProgress<C, MockDaSpec> = chain_state_ref
            .get_in_progress_transition(&mut kernel_working_set)
            .unwrap();

        assert_eq!(
            new_tx_in_progress,
            TransitionInProgress::<C, MockDaSpec>::new(
                MockHash::from([10; 32]),
                MockValidityCond::default(),
                GasPrice::ZEROED,
                Gas::zero()
            ),
            "The new transition has not been correctly stored"
        );

        assert!(has_tx_events(&apply_blob_outcome),);
    }

    // We apply a new transaction with the same values
    let new_slot_data: MockBlock = MockBlock {
        header: MockBlockHeader {
            prev_hash: [10; 32].into(),
            hash: [20; 32].into(),
            height: 1,
            time: Time::now(),
        },
        validity_cond: MockValidityCond::default(),
        blobs: Default::default(),
    };

    let storage = storage_manager.create_storage();
    let result = stf.apply_slot(
        &new_root_hash,
        storage,
        Default::default(),
        &new_slot_data.header,
        &new_slot_data.validity_cond,
        &mut [blob],
    );
    storage_manager.commit(result.change_set);

    #[cfg(test)]
    {
        assert_eq!(1, result.batch_receipts.len());
        let apply_blob_outcome = result.batch_receipts[0].clone();
        assert_eq!(
            SequencerOutcome::Rewarded(0),
            apply_blob_outcome.inner,
            "Sequencer execution should have succeeded but failed "
        );

        let storage = storage_manager.create_storage();
        // Computes the new working set after slot application
        let mut working_set = WorkingSet::new(storage);

        let chain_state_ref: &ChainState<C, MockDaSpec> = test_kernel.chain_state();

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
        let new_tx_in_progress: TransitionInProgress<C, MockDaSpec> = chain_state_ref
            .get_in_progress_transition(&mut kernel_working_set)
            .unwrap();

        assert_eq!(
            new_tx_in_progress,
            TransitionInProgress::<C, MockDaSpec>::new(
                [20; 32].into(),
                MockValidityCond::default(),
                GasPrice::ZEROED,
                Gas::zero()
            ),
            "The new transition has not been correctly stored"
        );
    }

    // TODO(@theochap):
    // To fix
    // let last_tx_stored: StateTransitionId<C, MockDaSpec> = chain_state_ref
    //     .get_historical_transitions(1, &mut working_set)
    //     .unwrap();

    // assert_eq!(
    //     last_tx_stored,
    //     StateTransitionId::new(
    //         [10; 32].into(),
    //         new_root_hash,
    //         MockValidityCond::default(),
    //         Gas::zero(),
    //         Gas::zero()
    //     )
    // );
}
