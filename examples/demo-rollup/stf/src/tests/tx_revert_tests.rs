use sov_accounts::Response;
use sov_mock_da::{MockAddress, MockBlock, MockDaSpec, MOCK_SEQUENCER_DA_ADDRESS};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::runtime::capabilities::FatalError;
use sov_modules_api::{PrivateKey, PublicKey, Spec, WorkingSet};
use sov_modules_stf_blueprint::{SequencerOutcome, StfBlueprint, TxEffect};
use sov_prover_storage_manager::ProverStorageManager;
use sov_rollup_interface::da::RelevantBlobs;
use sov_rollup_interface::services::da::SlotData;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_state::DefaultStorageSpec;
use sov_test_utils::bank_data::get_default_token_id;
use sov_test_utils::{has_tx_events, new_test_blob_from_batch, TestHasher, TestSpec};

use super::{
    create_genesis_config_for_tests, create_storage_manager_for_tests, read_private_keys,
    RuntimeTest,
};
use crate::runtime::Runtime;
use crate::tests::da_simulation::{
    simulate_da_with_bad_nonce, simulate_da_with_bad_serialization, simulate_da_with_bad_sig,
    simulate_da_with_revert_msg,
};
use crate::tests::StfBlueprintTest;

// Assume there was a proper address and we converted it to bytes already.
const SEQUENCER_DA_ADDRESS: [u8; 32] = [1; 32];

#[test]
fn test_tx_revert() {
    // Test checks:
    //  - Batch is successfully applied even with incorrect txs
    //  - Nonce for bad transactions has increased

    let tempdir = tempfile::tempdir().unwrap();

    let config = create_genesis_config_for_tests();
    let sequencer_rollup_address = config.runtime.sequencer_registry.seq_rollup_address;

    let genesis_block = MockBlock::default();
    let block_1 = genesis_block.next_mock();
    let admin_key = read_private_keys::<TestSpec>().token_deployer.private_key;
    let admin_address: <TestSpec as Spec>::Address = admin_key.to_address();

    let storage = {
        let mut storage_manager = create_storage_manager_for_tests(tempdir.path());
        let stf: StfBlueprintTest = StfBlueprint::new();

        let (stf_state, ledger_state) = storage_manager
            .create_state_for(genesis_block.header())
            .unwrap();
        let (genesis_root, stf_state) = stf.init_chain(stf_state, config);
        storage_manager
            .save_change_set(genesis_block.header(), stf_state, ledger_state.into())
            .unwrap();

        let txs = simulate_da_with_revert_msg(admin_key.clone());
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
            SequencerOutcome::Rewarded(0),
            apply_blob_outcome.inner,
            "Unexpected outcome: Batch execution should have succeeded",
        );

        let txn_receipts = apply_block_result.batch_receipts[0].tx_receipts.clone();
        // 3 transactions
        // create 1000 tokens
        // transfer 15 tokens
        // transfer 5000 tokens // this should be reverted
        assert_eq!(txn_receipts[0].receipt, TxEffect::Successful);
        assert_eq!(txn_receipts[1].receipt, TxEffect::Successful);
        assert_eq!(txn_receipts[2].receipt, TxEffect::Reverted);

        storage_manager
            .save_change_set(
                block_1.header(),
                apply_block_result.change_set,
                ledger_state.into(),
            )
            .unwrap();
        let (storage, _) = storage_manager
            .create_state_after(block_1.header())
            .unwrap();
        storage
    };

    // Checks on storage after execution
    {
        let runtime = &mut Runtime::<TestSpec, MockDaSpec>::default();
        let mut working_set = WorkingSet::new(storage);
        let resp = runtime
            .bank
            .balance_of(
                None,
                admin_address,
                get_default_token_id::<TestSpec>(&admin_address),
                &mut working_set,
            )
            .unwrap();

        assert_eq!(resp.amount, Some(985));

        let resp = runtime
            .sequencer_registry
            .sequencer_address(
                MockAddress::from(MOCK_SEQUENCER_DA_ADDRESS),
                &mut working_set,
            )
            .unwrap();
        // Sequencer is not excluded from the list of allowed!
        assert_eq!(Some(sequencer_rollup_address), resp.address);

        let nonce = match runtime
            .accounts
            .get_account(
                admin_key.pub_key().secure_hash::<TestHasher>(),
                &mut working_set,
            )
            .unwrap()
        {
            Response::AccountExists { nonce, .. } => nonce,
            Response::AccountEmpty => 0,
        };

        // with 3 transactions, the final nonce should be 3
        // 0 -> 1
        // 1 -> 2
        // 2 -> 3
        // The minter account should have its nonce increased for 3 transactions
        assert_eq!(3, nonce);
    }
}

#[test]
fn test_tx_bad_signature() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path();

    let config = create_genesis_config_for_tests();

    let genesis_block = MockBlock::default();
    let block_1 = genesis_block.next_mock();
    let admin_key = read_private_keys::<TestSpec>().token_deployer.private_key;
    let storage = {
        let mut storage_manager = create_storage_manager_for_tests(path);
        let stf: StfBlueprintTest = StfBlueprint::new();
        let (stf_state, ledger_state) = storage_manager
            .create_state_for(genesis_block.header())
            .unwrap();
        let (genesis_root, stf_state) = stf.init_chain(stf_state, config);
        storage_manager
            .save_change_set(genesis_block.header(), stf_state, ledger_state.into())
            .unwrap();

        let txs = simulate_da_with_bad_sig(admin_key.clone());

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
            SequencerOutcome::Slashed(
                FatalError::SigVerificationFailed(
                    "Bad signature signature error: Verification equation was not satisfied".to_string()
                ),
            ),
            apply_blob_outcome.inner,
            "Unexpected outcome: Stateless verification should have failed due to invalid signature"
        );

        // The batch receipt contains no events.
        assert!(!has_tx_events(&apply_blob_outcome));
        storage_manager
            .save_change_set(
                block_1.header(),
                apply_block_result.change_set,
                ledger_state.into(),
            )
            .unwrap();
        let (storage, _) = storage_manager
            .create_state_after(block_1.header())
            .unwrap();
        storage
    };

    {
        let runtime = &mut Runtime::<TestSpec, MockDaSpec>::default();
        let mut working_set = WorkingSet::new(storage);
        let nonce = match runtime
            .accounts
            .get_account(
                admin_key.pub_key().secure_hash::<TestHasher>(),
                &mut working_set,
            )
            .unwrap()
        {
            Response::AccountExists { nonce, .. } => nonce,
            Response::AccountEmpty => 0,
        };
        assert_eq!(0, nonce);
    }
}

fn get_attester_stake_for_block(
    block: &MockBlock,
    storage_manager: &mut ProverStorageManager<MockDaSpec, DefaultStorageSpec>,
    stf: &StfBlueprintTest,
) -> u64 {
    let (stf_state, _ledger_state) = storage_manager.create_state_for(block.header()).unwrap();

    let mut working_set = WorkingSet::new(stf_state);
    stf.runtime()
        .sequencer_registry
        .get_sender_balance(&(MOCK_SEQUENCER_DA_ADDRESS.into()), &mut working_set)
        .expect("The sequencer should be registered")
}

/// This test ensures that the sequencer gets penalized for submitting a proof that has a wrong nonce.
#[test]
fn test_tx_bad_nonce() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path();

    let config = create_genesis_config_for_tests();
    let genesis_block = MockBlock::default();
    let block_1 = genesis_block.next_mock();
    let block_2 = block_1.next_mock();
    let admin_key = read_private_keys::<TestSpec>().token_deployer.private_key;
    {
        let mut storage_manager = create_storage_manager_for_tests(path);
        let stf: StfBlueprintTest = StfBlueprint::new();
        let (stf_state, ledger_state) = storage_manager
            .create_state_for(genesis_block.header())
            .unwrap();
        let (genesis_root, stf_state) = stf.init_chain(stf_state, config);
        storage_manager
            .save_change_set(genesis_block.header(), stf_state, ledger_state.into())
            .unwrap();

        let txs = simulate_da_with_bad_nonce(admin_key);

        let blob = new_test_blob_from_batch(
            BatchWithId { txs, id: [0; 32] },
            &MOCK_SEQUENCER_DA_ADDRESS,
            [0; 32],
        );

        let mut relevant_blobs = RelevantBlobs {
            proof_blobs: Default::default(),
            batch_blobs: vec![blob],
        };

        let initial_sequencer_stake =
            get_attester_stake_for_block(&block_1, &mut storage_manager, &stf);

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
        let tx_receipts = apply_block_result.batch_receipts[0].tx_receipts.clone();
        // Bad nonce means that the transaction has to be reverted
        assert_eq!(tx_receipts[0].receipt, TxEffect::Duplicate);

        // We don't slash the sequencer for a bad nonce, since the nonce change might have
        // happened while the transaction was in-flight. However, we do *penalize* the sequencer
        // in this case.
        // We're asserting that here to track if the logic changes

        // Since the sequencer is penalized, he is rewarded with 0 tokens.
        let sequencer_outcome = apply_block_result.batch_receipts[0].inner.clone();
        match sequencer_outcome {
            SequencerOutcome::Rewarded(amount) => assert!(amount == 0),
            _ => panic!("Sequencer should have been penalized"),
        }

        // We can check that the sequencer staked amount went down.
        storage_manager
            .save_change_set(
                block_1.header(),
                apply_block_result.change_set,
                ledger_state.into(),
            )
            .expect("Saving the change set should not fail");

        let final_sequencer_stake =
            get_attester_stake_for_block(&block_2, &mut storage_manager, &stf);

        assert!(
            final_sequencer_stake < initial_sequencer_stake,
            "The sequencer stake should have decreased, final_sequencer_stake = {:?}, initial_sequencer_stake = {:?}",
            final_sequencer_stake, initial_sequencer_stake
        );
    }
}

#[test]
fn test_tx_bad_serialization() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path();

    let config = create_genesis_config_for_tests();
    let sequencer_rollup_address = config.runtime.sequencer_registry.seq_rollup_address;

    let genesis_block = MockBlock::default();
    let block_1 = genesis_block.next_mock();
    let mut storage_manager = create_storage_manager_for_tests(path);
    let admin_key = read_private_keys::<TestSpec>().token_deployer.private_key;

    let (genesis_root, sequencer_balance_before) = {
        let stf: StfBlueprintTest = StfBlueprint::new();

        let (stf_state, ledger_state) = storage_manager
            .create_state_for(genesis_block.header())
            .unwrap();
        let (genesis_root, stf_state) = stf.init_chain(stf_state, config);
        storage_manager
            .save_change_set(genesis_block.header(), stf_state, ledger_state.into())
            .unwrap();

        let balance = {
            let (stf_state, _) = storage_manager
                .create_state_after(genesis_block.header())
                .unwrap();
            let runtime: RuntimeTest = Runtime::default();
            let mut working_set = WorkingSet::<TestSpec>::new(stf_state.clone());

            let coins = runtime
                .sequencer_registry
                .get_coins_to_lock(&mut working_set);

            runtime
                .bank
                .get_balance_of(&sequencer_rollup_address, coins.token_id, &mut working_set)
                .unwrap()
        };
        (genesis_root, balance)
    };

    let storage = {
        let stf: StfBlueprintTest = StfBlueprint::new();

        let txs = simulate_da_with_bad_serialization(admin_key.clone());
        let blob = new_test_blob_from_batch(
            BatchWithId { txs, id: [0; 32] },
            &MOCK_SEQUENCER_DA_ADDRESS,
            [0; 32],
        );

        let mut relevant_blobs = RelevantBlobs {
            proof_blobs: Default::default(),
            batch_blobs: vec![blob],
        };

        let (storage, ledger_state) = storage_manager.create_state_for(block_1.header()).unwrap();
        let apply_block_result = stf.apply_slot(
            &genesis_root,
            storage,
            Default::default(),
            &block_1.header,
            &block_1.validity_cond,
            relevant_blobs.as_iters(),
        );

        assert_eq!(1, apply_block_result.batch_receipts.len());
        let apply_blob_outcome = apply_block_result.batch_receipts[0].clone();

        assert_eq!(
            SequencerOutcome::Slashed (
                FatalError::MessageDecodingFailed("Unexpected variant index: 110".to_string(), [210, 84, 119, 49, 64, 12, 6, 68, 188, 255, 107, 181, 229, 18, 190, 134, 64, 112, 190, 131, 236, 116, 93, 23, 248, 247, 172, 189, 121, 235, 55, 106]),
            ),
            apply_blob_outcome.inner,
            "Unexpected outcome: Stateless verification should have failed due to invalid signature"
        );

        // The batch receipt contains no events.
        assert!(!has_tx_events(&apply_blob_outcome));
        storage_manager
            .save_change_set(
                block_1.header(),
                apply_block_result.change_set,
                ledger_state.into(),
            )
            .unwrap();
        let (storage, _) = storage_manager
            .create_state_after(block_1.header())
            .unwrap();
        storage
    };

    {
        let runtime = &mut Runtime::<TestSpec, MockDaSpec>::default();
        let mut working_set = WorkingSet::new(storage);

        // Sequencer is not in the list of allowed sequencers

        let allowed_sequencer = runtime
            .sequencer_registry
            .sequencer_address(MockAddress::from(SEQUENCER_DA_ADDRESS), &mut working_set)
            .unwrap();
        assert!(allowed_sequencer.address.is_none());

        // Balance of sequencer is not increased
        let coins = runtime
            .sequencer_registry
            .get_coins_to_lock(&mut working_set);
        let sequencer_balance_after = runtime
            .bank
            .get_balance_of(&sequencer_rollup_address, coins.token_id, &mut working_set)
            .unwrap();
        assert_eq!(sequencer_balance_before, sequencer_balance_after);
    }
}
