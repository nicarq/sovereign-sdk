use std::convert::Infallible;

use sov_bank::config_gas_token_id;
use sov_mock_da::{MockAddress, MockBlock, MOCK_SEQUENCER_DA_ADDRESS};
use sov_modules_api::{
    ApiStateAccessor, Batch, BatchSequencerOutcome, ExecutionContext, PrivateKey, PublicKey, Spec,
};
use sov_modules_stf_blueprint::{StfBlueprint, TxProcessingError};
use sov_rollup_interface::da::RelevantBlobs;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_test_utils::generators::bank::get_default_token_id;
use sov_test_utils::storage::SimpleStorageManager;
use sov_test_utils::{TestHasher, TestSpec, TestStorageSpec};

use super::{create_genesis_config_for_tests, read_private_keys, RuntimeTest};
use crate::runtime::Runtime;
use crate::tests::da_simulation::{
    simulate_da_with_bad_nonce, simulate_da_with_bad_serialization, simulate_da_with_bad_sig,
    simulate_da_with_revert_msg,
};
use crate::tests::{default_rewards, has_tx_events, new_test_blob_from_batch, StfBlueprintTest};

// Assume there was a proper address and we converted it to bytes already.
const SEQUENCER_DA_ADDRESS: [u8; 32] = [1; 32];

fn assert_outcome(outcome: &BatchSequencerOutcome) {
    match outcome {
        BatchSequencerOutcome::Executed(rewards) => {
            assert_eq!(rewards.accumulated_reward, 0);
            assert!(rewards.accumulated_penalty > 0);
            assert_eq!(rewards.hooks_cost, 0);
        }
        BatchSequencerOutcome::Ignored(_) => todo!(),
    }
}

#[test]
fn test_tx_revert() -> Result<(), Infallible> {
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
        let mut storage_manager = SimpleStorageManager::<TestStorageSpec>::new(tempdir.path());
        let stf: StfBlueprintTest = StfBlueprint::new();

        let stf_state = storage_manager.create_storage();
        let (genesis_root, stf_changes) =
            stf.init_chain(&Default::default(), &Default::default(), stf_state, config);
        storage_manager.commit(stf_changes);

        let txs = simulate_da_with_revert_msg(admin_key.clone());
        let blob = new_test_blob_from_batch(Batch { txs }, &MOCK_SEQUENCER_DA_ADDRESS);

        let mut relevant_blobs = RelevantBlobs {
            proof_blobs: Default::default(),
            batch_blobs: vec![blob],
        };

        let stf_state = storage_manager.create_storage();
        let apply_block_result = stf.apply_slot(
            &genesis_root,
            stf_state,
            Default::default(),
            &block_1.header,
            &block_1.validity_cond,
            relevant_blobs.as_iters(),
            ExecutionContext::Node,
        );

        assert_eq!(1, apply_block_result.batch_receipts.len());
        let apply_blob_outcome = apply_block_result.batch_receipts[0].clone();

        assert_eq!(
            BatchSequencerOutcome::Executed(default_rewards()),
            apply_blob_outcome.inner.outcome,
            "Sequencer execution should have succeeded but failed "
        );

        let txn_receipts = apply_block_result.batch_receipts[0].tx_receipts.clone();
        // 3 transactions
        // create 1000 tokens
        // transfer 15 tokens
        // transfer 5000 tokens // this should be reverted
        assert!(txn_receipts[0].receipt.is_successful());
        assert!(txn_receipts[1].receipt.is_successful());
        assert!(txn_receipts[2].receipt.is_reverted());

        storage_manager.commit(apply_block_result.change_set);
        storage_manager.create_storage()
    };

    // Checks on storage after execution
    {
        let runtime = &mut Runtime::<TestSpec>::default();
        let mut state = ApiStateAccessor::from_storage(storage, runtime);

        let resp = runtime
            .bank
            .get_balance_of(
                &admin_address,
                get_default_token_id::<TestSpec>(&admin_address),
                &mut state,
            )
            .unwrap();

        assert_eq!(resp, Some(985));

        let resp = runtime
            .sequencer_registry
            .get_sequencer_address(MockAddress::from(MOCK_SEQUENCER_DA_ADDRESS), &mut state)
            .unwrap();
        // Sequencer is not excluded from the list of allowed!
        assert_eq!(Some(sequencer_rollup_address), resp);

        let nonce = runtime
            .nonces
            .nonce(
                &admin_key.pub_key().credential_id::<TestHasher>(),
                &mut state,
            )?
            .unwrap();

        // with 3 transactions, the final nonce should be 3
        // 0 -> 1
        // 1 -> 2
        // 2 -> 3
        // The minter account should have its nonce increased for 3 transactions
        assert_eq!(3, nonce);
    }

    Ok(())
}

#[test]
fn test_tx_bad_signature() -> Result<(), Infallible> {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path();

    let config = create_genesis_config_for_tests();

    let genesis_block = MockBlock::default();
    let block_1 = genesis_block.next_mock();
    let admin_key = read_private_keys::<TestSpec>().token_deployer.private_key;
    let storage = {
        let mut storage_manager = SimpleStorageManager::<TestStorageSpec>::new(path);
        let stf: StfBlueprintTest = StfBlueprint::new();
        let stf_state = storage_manager.create_storage();
        let (genesis_root, stf_changes) =
            stf.init_chain(&Default::default(), &Default::default(), stf_state, config);
        storage_manager.commit(stf_changes);

        let txs = simulate_da_with_bad_sig(admin_key.clone());

        let blob = new_test_blob_from_batch(Batch { txs }, &MOCK_SEQUENCER_DA_ADDRESS);

        let mut relevant_blobs = RelevantBlobs {
            proof_blobs: Default::default(),
            batch_blobs: vec![blob],
        };

        let stf_state = storage_manager.create_storage();
        let apply_block_result = stf.apply_slot(
            &genesis_root,
            stf_state,
            Default::default(),
            &block_1.header,
            &block_1.validity_cond,
            relevant_blobs.as_iters(),
            ExecutionContext::Node,
        );

        assert_eq!(1, apply_block_result.batch_receipts.len());

        let batch_receipt = &apply_block_result.batch_receipts[0];

        assert_outcome(&batch_receipt.inner.outcome);

        let tx_receipts = &batch_receipt.tx_receipts;

        assert_eq!(tx_receipts.len(), 1);

        match &tx_receipts[0].receipt {
            sov_modules_api::TxEffect::Skipped(skipped) => assert_eq!(
                skipped.error,
                TxProcessingError::AuthenticationFailed("Authentication failed for tx: 0xcb9c1de47ae8f504c9eaf58ae0200a3287115a5f9d57ed69be9baa20315d7acb. Error: Signature verification failed: Invalid signature: signature error: Verification equation was not satisfied".to_string())
            ),
            unexpected => panic!("Expected TxEffect::Skipped but got {:?}", unexpected),
        }

        assert_outcome(&batch_receipt.inner.outcome);

        // The batch receipt contains no events.
        assert!(!has_tx_events(batch_receipt));
        storage_manager.commit(apply_block_result.change_set);
        storage_manager.create_storage()
    };

    {
        let runtime = &mut Runtime::<TestSpec>::default();
        let mut state = ApiStateAccessor::from_storage(storage, runtime);

        let nonce = runtime
            .nonces
            .nonce(
                &admin_key.pub_key().credential_id::<TestHasher>(),
                &mut state,
            )?
            .unwrap_or_default();

        assert_eq!(0, nonce);
    }

    Ok(())
}

fn get_attester_stake_for_block(
    storage_manager: &mut SimpleStorageManager<TestStorageSpec>,
    stf: &StfBlueprintTest,
) -> Result<u64, Infallible> {
    let stf_state = storage_manager.create_storage();

    let mut state = ApiStateAccessor::from_storage(stf_state, stf.runtime());

    Ok(stf
        .runtime()
        .sequencer_registry
        .get_sender_balance(&(MOCK_SEQUENCER_DA_ADDRESS.into()), &mut state)?
        .expect("The sequencer should be registered"))
}

/// This test ensures that the sequencer gets penalized for submitting a proof that has a wrong nonce.
#[test]
fn test_tx_bad_nonce() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path();

    let config = create_genesis_config_for_tests();
    let genesis_block = MockBlock::default();
    let block_1 = genesis_block.next_mock();
    let admin_key = read_private_keys::<TestSpec>().token_deployer.private_key;
    {
        let mut storage_manager = SimpleStorageManager::<TestStorageSpec>::new(path);
        let stf: StfBlueprintTest = StfBlueprint::new();
        let stf_state = storage_manager.create_storage();
        let (genesis_root, stf_state) =
            stf.init_chain(&Default::default(), &Default::default(), stf_state, config);
        storage_manager.commit(stf_state);

        let txs = simulate_da_with_bad_nonce(admin_key);

        let blob = new_test_blob_from_batch(Batch { txs }, &MOCK_SEQUENCER_DA_ADDRESS);

        let mut relevant_blobs = RelevantBlobs {
            proof_blobs: Default::default(),
            batch_blobs: vec![blob],
        };

        let initial_sequencer_stake = get_attester_stake_for_block(&mut storage_manager, &stf);

        let stf_state = storage_manager.create_storage();

        let apply_block_result = stf.apply_slot(
            &genesis_root,
            stf_state,
            Default::default(),
            &block_1.header,
            &block_1.validity_cond,
            relevant_blobs.as_iters(),
            ExecutionContext::Node,
        );

        // When the nonce is not correct, the transaction receipt does not appear in the block
        assert_eq!(1, apply_block_result.batch_receipts.len());
        let tx_receipts = apply_block_result.batch_receipts[0].tx_receipts.clone();
        // Bad nonce means that the transaction has to be reverted

        match &tx_receipts[0].receipt {
            sov_modules_api::TxEffect::Skipped(skipped) => {
                assert_eq!(skipped.error, TxProcessingError::IncorrectNonce(
                    "Tx bad nonce for credential id: 0xfea6ac5b8751120fb62fff67b54d2eac66aef307c7dde1d394dea1e09e43dd44, expected: 0, but found: 18446744073709551615".to_string()
                ));
            }
            _ => panic!(
                "Expected Skipped error, but got a different TxEffect: {:?}",
                tx_receipts[0].receipt
            ),
        }

        // We don't slash the sequencer for a bad nonce, since the nonce change might have
        // happened while the transaction was in-flight. However, we do *penalize* the sequencer
        // in this case.
        // We're asserting that here to track if the logic changes

        // Since the sequencer is penalized, he is rewarded with 0 tokens.
        let sequencer_outcome = apply_block_result.batch_receipts[0].inner.clone().outcome;
        assert_outcome(&sequencer_outcome);
        // We can check that the sequencer staked amount went down.
        storage_manager.commit(apply_block_result.change_set);

        let final_sequencer_stake = get_attester_stake_for_block(&mut storage_manager, &stf);

        assert!(
            final_sequencer_stake < initial_sequencer_stake,
            "The sequencer stake should have decreased, final_sequencer_stake = {:?}, initial_sequencer_stake = {:?}",
            final_sequencer_stake, initial_sequencer_stake
        );
    }
}

#[test]
fn test_tx_bad_serialization() -> Result<(), Infallible> {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path();

    let config = create_genesis_config_for_tests();
    let sequencer_rollup_address = config.runtime.sequencer_registry.seq_rollup_address;

    let genesis_block = MockBlock::default();
    let block_1 = genesis_block.next_mock();
    let mut storage_manager = SimpleStorageManager::<TestStorageSpec>::new(path);
    let admin_key = read_private_keys::<TestSpec>().token_deployer.private_key;

    let (genesis_root, sequencer_balance_before) = {
        let stf: StfBlueprintTest = StfBlueprint::new();

        let stf_state = storage_manager.create_storage();
        let (genesis_root, stf_changes) =
            stf.init_chain(&Default::default(), &Default::default(), stf_state, config);
        storage_manager.commit(stf_changes);

        let balance = {
            let stf_state = storage_manager.create_storage();
            let runtime: RuntimeTest = Runtime::default();
            let mut state = ApiStateAccessor::from_storage(stf_state.clone(), &runtime);

            runtime
                .bank
                .get_balance_of(&sequencer_rollup_address, config_gas_token_id(), &mut state)?
                .unwrap()
        };
        (genesis_root, balance)
    };

    let storage = {
        let stf: StfBlueprintTest = StfBlueprint::new();

        let txs = simulate_da_with_bad_serialization(admin_key.clone());
        let blob = new_test_blob_from_batch(Batch { txs }, &MOCK_SEQUENCER_DA_ADDRESS);

        let mut relevant_blobs = RelevantBlobs {
            proof_blobs: Default::default(),
            batch_blobs: vec![blob],
        };

        let storage = storage_manager.create_storage();
        let apply_block_result = stf.apply_slot(
            &genesis_root,
            storage,
            Default::default(),
            &block_1.header,
            &block_1.validity_cond,
            relevant_blobs.as_iters(),
            ExecutionContext::Node,
        );

        assert_eq!(1, apply_block_result.batch_receipts.len());

        let batch_receipt = &apply_block_result.batch_receipts[0];

        assert_outcome(&batch_receipt.inner.outcome);

        let tx_receipts = &batch_receipt.tx_receipts;

        assert_eq!(tx_receipts.len(), 1);

        match &tx_receipts[0].receipt {
            sov_modules_api::TxEffect::Skipped(skipped) => assert_eq!(
                skipped.error,
                TxProcessingError::AuthenticationFailed("Authentication failed for tx: 0xb5673952f72e0b4c1db5b9594c5ad8d0c7eaf50bcdfca9bda0b27fe2212dab60. Error: Transaction decoding error: IO error: Unexpected variant tag: 110".to_string())
            ),
            unexpected => panic!("Expected TxEffect::Skipped but got {:?}", unexpected),
        }

        assert_outcome(&batch_receipt.inner.outcome);

        // The batch receipt contains no events.
        assert!(!has_tx_events(batch_receipt));
        storage_manager.commit(apply_block_result.change_set);
        storage_manager.create_storage()
    };

    {
        let runtime = &mut Runtime::<TestSpec>::default();
        let mut state = ApiStateAccessor::from_storage(storage, runtime);

        // Sequencer is not in the list of allowed sequencers

        let allowed_sequencer = runtime
            .sequencer_registry
            .get_sequencer_address(MockAddress::from(SEQUENCER_DA_ADDRESS), &mut state)
            .unwrap();
        assert_eq!(None, allowed_sequencer);

        let sequencer_balance_after = runtime
            .bank
            .get_balance_of(&sequencer_rollup_address, config_gas_token_id(), &mut state)?
            .unwrap();
        assert_eq!(sequencer_balance_before, sequencer_balance_after);
    }

    Ok(())
}
