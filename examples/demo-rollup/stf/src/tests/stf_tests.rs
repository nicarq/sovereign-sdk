use std::vec;

use sov_mock_da::{MockBlock, MockDaSpec, MOCK_SEQUENCER_DA_ADDRESS};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::transaction::SequencerReward;
use sov_modules_api::{ApiStateAccessor, Spec};
use sov_modules_stf_blueprint::{BatchSequencerOutcome, StfBlueprint};
use sov_rollup_interface::da::RelevantBlobs;
use sov_rollup_interface::services::da::SlotData;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_test_utils::bank_data::get_default_token_id;
use sov_test_utils::{has_tx_events, new_test_blob_from_batch, TestSpec};

use crate::runtime::Runtime;
use crate::tests::da_simulation::simulate_da;
use crate::tests::{
    create_genesis_config_for_tests, create_storage_manager_for_tests, read_private_keys,
    StfBlueprintTest, S,
};

#[test]
fn test_demo_values_in_db() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path();
    let mut storage_manager = create_storage_manager_for_tests(path);
    let config = create_genesis_config_for_tests();

    let genesis_block = MockBlock::default();
    let block_1 = genesis_block.next_mock();

    let admin_key_and_address = read_private_keys::<TestSpec>().tx_signer;
    let admin_address: <TestSpec as Spec>::Address = admin_key_and_address.address;
    let admin_private_key = admin_key_and_address.private_key;

    let last_block = {
        let stf: StfBlueprintTest = StfBlueprint::new();
        let (stf_state, ledger_state) = storage_manager
            .create_state_for(genesis_block.header())
            .unwrap();
        let (genesis_root, stf_change_set) = stf.init_chain(stf_state, config);
        storage_manager
            .save_change_set(genesis_block.header(), stf_change_set, ledger_state.into())
            .unwrap();

        let txs = simulate_da(admin_private_key);
        let blob = new_test_blob_from_batch(
            BatchWithId { txs, id: [0; 32] },
            &MOCK_SEQUENCER_DA_ADDRESS,
            [0; 32],
        );

        let mut relevant_blobs = RelevantBlobs {
            proof_blobs: Default::default(),
            batch_blobs: vec![blob],
        };

        let (stf_state, ledger_state) = storage_manager.create_state_for(block_1.header()).unwrap();

        let result = stf.apply_slot(
            &genesis_root,
            stf_state,
            Default::default(),
            &block_1.header,
            &block_1.validity_cond,
            relevant_blobs.as_iters(),
        );
        assert_eq!(1, result.batch_receipts.len());
        // 2 transactions from value setter
        // 2 transactions from bank
        assert_eq!(4, result.batch_receipts[0].tx_receipts.len());

        let apply_blob_outcome = result.batch_receipts[0].clone();
        assert_eq!(
            BatchSequencerOutcome::Rewarded(SequencerReward::ZERO),
            apply_blob_outcome.inner,
            "Sequencer execution should have succeeded but failed "
        );

        assert!(has_tx_events(&apply_blob_outcome),);
        storage_manager
            .save_change_set(block_1.header(), result.change_set, ledger_state.into())
            .unwrap();
        block_1
    };

    // Generate a new storage instance after dumping data to the db.
    {
        let next_block = last_block.next_mock();
        let runtime = &mut Runtime::<TestSpec, MockDaSpec>::default();
        let (stf_state, _ledger_state) = storage_manager
            .create_state_for(next_block.header())
            .unwrap();
        let mut api_state_accessor = ApiStateAccessor::new(stf_state);
        let resp = runtime
            .bank
            .supply_of(
                None,
                get_default_token_id::<S>(&admin_address),
                &mut api_state_accessor,
            )
            .unwrap();
        assert_eq!(resp, sov_bank::TotalSupplyResponse { amount: Some(1000) });

        assert_eq!(
            runtime.value_setter.value.get(&mut api_state_accessor),
            Some(33)
        );
    }
}

#[test]
fn test_demo_values_in_cache() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path();
    let mut storage_manager = create_storage_manager_for_tests(path);

    let stf: StfBlueprintTest = StfBlueprint::new();

    let config = create_genesis_config_for_tests();

    let genesis_block = MockBlock::default();
    let (stf_state, ledger_state) = storage_manager
        .create_state_for(genesis_block.header())
        .unwrap();
    let (genesis_root, stf_state) = stf.init_chain(stf_state, config);
    storage_manager
        .save_change_set(genesis_block.header(), stf_state, ledger_state.into())
        .unwrap();

    let admin_private_key_and_address = read_private_keys::<TestSpec>().tx_signer;
    let admin_private_key = admin_private_key_and_address.private_key;
    let admin_address: <S as Spec>::Address = admin_private_key_and_address.address;

    let txs = simulate_da(admin_private_key);

    let blob = new_test_blob_from_batch(
        BatchWithId { txs, id: [0; 32] },
        &MOCK_SEQUENCER_DA_ADDRESS,
        [0; 32],
    );

    let mut relevant_blobs = RelevantBlobs {
        proof_blobs: Default::default(),
        batch_blobs: vec![blob],
    };

    let block_1 = genesis_block.next_mock();
    let (stf_state, ledger_state) = storage_manager.create_state_for(block_1.header()).unwrap();

    let apply_block_result = stf.apply_slot(
        &genesis_root,
        stf_state,
        Default::default(),
        &block_1.header,
        &block_1.validity_cond,
        relevant_blobs.as_iters(),
    );

    assert_eq!(1, apply_block_result.batch_receipts.len());
    let apply_blob_outcome = apply_block_result.batch_receipts[0].clone();

    assert_eq!(
        BatchSequencerOutcome::Rewarded(SequencerReward::ZERO),
        apply_blob_outcome.inner,
        "Sequencer execution should have succeeded but failed"
    );

    assert!(has_tx_events(&apply_blob_outcome),);

    let runtime = &mut Runtime::<TestSpec, MockDaSpec>::default();

    storage_manager
        .save_change_set(
            block_1.header(),
            apply_block_result.change_set,
            ledger_state.into(),
        )
        .unwrap();

    let (stf_storage, _) = storage_manager
        .create_state_after(block_1.header())
        .unwrap();

    let mut api_state_accessor = ApiStateAccessor::new(stf_storage);

    let resp = runtime
        .bank
        .supply_of(
            None,
            get_default_token_id::<S>(&admin_address),
            &mut api_state_accessor,
        )
        .unwrap();
    assert_eq!(resp, sov_bank::TotalSupplyResponse { amount: Some(1000) });

    assert_eq!(
        runtime.value_setter.value.get(&mut api_state_accessor),
        Some(33)
    );
}

#[test]
fn test_sequencer_unknown_sequencer() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path();

    let mut config = create_genesis_config_for_tests();
    config.runtime.sequencer_registry.is_preferred_sequencer = false;

    let genesis_block = MockBlock::default();
    let block_1 = genesis_block.next_mock();

    let mut storage_manager = create_storage_manager_for_tests(path);
    let stf: StfBlueprintTest = StfBlueprint::new();
    let (stf_state, ledger_state) = storage_manager
        .create_state_for(genesis_block.header())
        .unwrap();
    let (genesis_root, stf_state) = stf.init_chain(stf_state, config);
    storage_manager
        .save_change_set(genesis_block.header(), stf_state, ledger_state.into())
        .unwrap();

    let some_sequencer: [u8; 32] = [121; 32];

    let private_key = read_private_keys::<TestSpec>().tx_signer.private_key;
    let txs = simulate_da(private_key);
    let blob = new_test_blob_from_batch(BatchWithId { txs, id: [0; 32] }, &some_sequencer, [0; 32]);

    let mut relevant_blobs = RelevantBlobs {
        proof_blobs: Default::default(),
        batch_blobs: vec![blob],
    };

    let (stf_state, _ledger_state) = storage_manager.create_state_for(block_1.header()).unwrap();

    let apply_block_result = stf.apply_slot(
        &genesis_root,
        stf_state,
        Default::default(),
        &block_1.header,
        &block_1.validity_cond,
        relevant_blobs.as_iters(),
    );

    // The sequencer isn't registered, so the blob should be ignored.
    assert_eq!(0, apply_block_result.batch_receipts.len());
}
