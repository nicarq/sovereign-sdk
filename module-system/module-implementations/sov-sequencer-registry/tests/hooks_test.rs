use helpers::*;
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::ApplyBatchHooks;
use sov_modules_api::{Context, Module, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;
use sov_sequencer_registry::{CallMessage, SequencerOutcome, SequencerRegistry};

mod helpers;

#[test]
fn begin_blob_hook_known_sequencer() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    let balance_after_genesis = {
        let resp = test_sequencer.query_balance_via_bank(working_set).unwrap();
        resp.amount.unwrap()
    };
    assert_eq!(INITIAL_BALANCE - LOCKED_AMOUNT, balance_after_genesis);

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };

    test_sequencer
        .registry
        .begin_batch_hook(&mut test_batch, &genesis_sequencer_da_address, working_set)
        .unwrap();

    let resp = test_sequencer.query_balance_via_bank(working_set).unwrap();
    assert_eq!(balance_after_genesis, resp.amount.unwrap());
    let resp = test_sequencer
        .registry
        .sequencer_address(genesis_sequencer_da_address, working_set)
        .unwrap();
    assert!(resp.address.is_some());
}

#[test]
fn begin_blob_hook_unknown_sequencer() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };

    let result = test_sequencer.registry.begin_batch_hook(
        &mut test_batch,
        &MockAddress::from(UNKNOWN_SEQUENCER_DA_ADDRESS),
        working_set,
    );
    assert!(result.is_err());
    let expected_message = format!(
        "sender {} is not allowed to submit blobs",
        MockAddress::from(UNKNOWN_SEQUENCER_DA_ADDRESS)
    );
    let actual_message = result.err().unwrap().to_string();
    assert_eq!(expected_message, actual_message);
}

#[test]
fn end_blob_hook_success() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);
    let balance_after_genesis = {
        let resp = test_sequencer.query_balance_via_bank(working_set).unwrap();
        resp.amount.unwrap()
    };
    assert_eq!(INITIAL_BALANCE - LOCKED_AMOUNT, balance_after_genesis);

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);
    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };

    test_sequencer
        .registry
        .begin_batch_hook(&mut test_batch, &genesis_sequencer_da_address, working_set)
        .unwrap();

    <SequencerRegistry<C, Da> as ApplyBatchHooks<MockDaSpec>>::end_batch_hook(
        &test_sequencer.registry,
        SequencerOutcome::Completed,
        working_set,
    )
    .unwrap();
    let resp = test_sequencer.query_balance_via_bank(working_set).unwrap();
    assert_eq!(balance_after_genesis, resp.amount.unwrap());
    let resp = test_sequencer
        .registry
        .sequencer_address(genesis_sequencer_da_address, working_set)
        .unwrap();
    assert!(resp.address.is_some());
}

#[test]
fn end_blob_hook_slash() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);
    let balance_after_genesis = {
        let resp = test_sequencer.query_balance_via_bank(working_set).unwrap();
        resp.amount.unwrap()
    };
    assert_eq!(INITIAL_BALANCE - LOCKED_AMOUNT, balance_after_genesis);

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };

    test_sequencer
        .registry
        .begin_batch_hook(&mut test_batch, &genesis_sequencer_da_address, working_set)
        .unwrap();

    let result = SequencerOutcome::Slashed {
        sequencer: genesis_sequencer_da_address,
    };
    <SequencerRegistry<C, Da> as ApplyBatchHooks<MockDaSpec>>::end_batch_hook(
        &test_sequencer.registry,
        result,
        working_set,
    )
    .unwrap();

    let resp = test_sequencer.query_balance_via_bank(working_set).unwrap();
    assert_eq!(balance_after_genesis, resp.amount.unwrap());
    let resp = test_sequencer
        .registry
        .sequencer_address(genesis_sequencer_da_address, working_set)
        .unwrap();
    assert!(resp.address.is_none());
}

#[test]
fn end_blob_hook_slash_preferred_sequencer() {
    let bank = sov_bank::Bank::<C>::default();
    let (bank_config, seq_rollup_address) = create_bank_config();

    let token_address = sov_bank::get_genesis_token_address::<C>(
        &bank_config.tokens[0].token_name,
        bank_config.tokens[0].salt,
    );

    let registry = SequencerRegistry::<C, Da>::default();
    let mut sequencer_config = create_sequencer_config(seq_rollup_address, token_address);

    sequencer_config.is_preferred_sequencer = true;

    let test_sequencer = TestSequencer {
        bank,
        bank_config,
        registry,
        sequencer_config,
    };

    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);
    let balance_after_genesis = {
        let resp = test_sequencer.query_balance_via_bank(working_set).unwrap();
        resp.amount.unwrap()
    };
    assert_eq!(INITIAL_BALANCE - LOCKED_AMOUNT, balance_after_genesis);

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };

    test_sequencer
        .registry
        .begin_batch_hook(&mut test_batch, &genesis_sequencer_da_address, working_set)
        .unwrap();

    let result = SequencerOutcome::Slashed {
        sequencer: genesis_sequencer_da_address,
    };
    <SequencerRegistry<C, Da> as ApplyBatchHooks<MockDaSpec>>::end_batch_hook(
        &test_sequencer.registry,
        result,
        working_set,
    )
    .unwrap();

    let resp = test_sequencer.query_balance_via_bank(working_set).unwrap();
    assert_eq!(balance_after_genesis, resp.amount.unwrap());
    let resp = test_sequencer
        .registry
        .sequencer_address(MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS), working_set)
        .unwrap();
    assert!(resp.address.is_none());

    assert!(test_sequencer
        .registry
        .get_preferred_sequencer(working_set)
        .is_none());
}

#[test]
fn end_blob_hook_slash_unknown_sequencer() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };
    let sequencer_address = MockAddress::from(UNKNOWN_SEQUENCER_DA_ADDRESS);
    test_sequencer
        .registry
        .begin_batch_hook(
            &mut test_batch,
            &MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS),
            working_set,
        )
        .unwrap();

    let resp = test_sequencer
        .registry
        .sequencer_address(sequencer_address, working_set)
        .unwrap();
    assert!(resp.address.is_none());

    let result = SequencerOutcome::Slashed {
        sequencer: sequencer_address,
    };
    <SequencerRegistry<C, Da> as ApplyBatchHooks<MockDaSpec>>::end_batch_hook(
        &test_sequencer.registry,
        result,
        working_set,
    )
    .unwrap();

    let resp = test_sequencer
        .registry
        .sequencer_address(sequencer_address, working_set)
        .unwrap();
    assert!(resp.address.is_none());
}

#[test]
fn begin_blob_hook_without_enough_stake() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    let mut test_blob = BatchWithId {
        txs: vec![],
        id: [0; 32],
    };

    test_sequencer
        .set_coins_amount_to_lock(LOCKED_AMOUNT + 1, working_set)
        .unwrap();

    let res = test_sequencer.registry.begin_batch_hook(
        &mut test_blob,
        &genesis_sequencer_da_address,
        working_set,
    );

    assert!(
        res.is_err(),
        "the staked required amount was increased; the genesis sequencer is out of balance"
    );
}

#[test]
fn slashed_sequencer_shouldnt_preserve_balance() {
    let test_sequencer = create_test_sequencer_large_balance();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    // created settings

    let initial_balance = test_sequencer.bank_config.tokens[0].address_and_balances[0].1;
    let deposit_amount = 100;
    let stake_amount = test_sequencer.sequencer_config.coins_to_lock.amount;
    let token_address = test_sequencer.sequencer_config.coins_to_lock.token_address;
    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    // sanity check the balance

    let genesis_sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);
    let balance_after_genesis = initial_balance - stake_amount;
    let balance = test_sequencer
        .bank
        .balance_of(None, genesis_sequencer_address, token_address, working_set)
        .unwrap()
        .amount
        .unwrap();
    assert_eq!(balance, balance_after_genesis);

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, working_set)
        .unwrap();
    assert_eq!(staked_balance, stake_amount);

    // deposit some additional stake amount

    let da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = C::new(genesis_sequencer_address, reward_address, 1);
    let deposit_message = CallMessage::Deposit {
        da_address: da_address.as_ref().to_vec(),
        amount: deposit_amount,
    };

    test_sequencer
        .registry
        .call(deposit_message, &sender_context, working_set)
        .expect("Sequencer deposit has failed");

    let balance_after_deposit = balance_after_genesis - deposit_amount;
    let balance = test_sequencer
        .bank
        .balance_of(None, genesis_sequencer_address, token_address, working_set)
        .unwrap()
        .amount
        .unwrap();
    assert_eq!(balance, balance_after_deposit);

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, working_set)
        .unwrap();

    assert_eq!(
        staked_balance,
        stake_amount + deposit_amount,
        "the deposit should be added to the staked amount"
    );

    // submit an invalid block and expect the sequencer to be slashed

    assert!(
        test_sequencer.query_if_sequencer_is_allowed(&genesis_sequencer_da_address, working_set),
    );

    let hash = [0u8; 32]; // invalid
    let result = SequencerOutcome::Slashed {
        sequencer: genesis_sequencer_da_address,
    };

    let mut test_blob = BatchWithId {
        txs: vec![],
        id: hash,
    };

    test_sequencer
        .registry
        .begin_batch_hook(&mut test_blob, &genesis_sequencer_da_address, working_set)
        .unwrap();

    test_sequencer
        .registry
        .end_batch_hook(result, working_set)
        .unwrap();

    assert!(
        !test_sequencer.query_if_sequencer_is_allowed(&genesis_sequencer_da_address, working_set),
        "the sequencer was slashed and shouldn't be allowed"
    );

    let balance = test_sequencer
        .bank
        .balance_of(None, genesis_sequencer_address, token_address, working_set)
        .unwrap()
        .amount
        .unwrap();

    assert_eq!(
        balance,
        balance_after_deposit,
        "the balance should be unchanged after slash; the slashed tokens are frozen on the registry account"
    );

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, working_set);
    assert!(staked_balance.is_none());

    // register the sequencer again and check the balances

    let register_message = CallMessage::Register {
        da_address: genesis_sequencer_da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };

    test_sequencer
        .registry
        .call(register_message, &sender_context, working_set)
        .expect("Sequencer registration has failed");

    let balance_after_re_register = balance_after_deposit - stake_amount;
    let balance = test_sequencer
        .bank
        .balance_of(None, genesis_sequencer_address, token_address, working_set)
        .unwrap()
        .amount
        .unwrap();

    assert_eq!(
        balance, balance_after_re_register,
        "the stake amount should be deducted from the sender account"
    );

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, working_set)
        .unwrap();

    assert_eq!(
        staked_balance, stake_amount,
        "the previous deposit should have been removed when the sequencer was slashed"
    );
}
