use sov_bank::GAS_TOKEN_ID;
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::ApplyBatchHooks;
use sov_modules_api::{Context, Module};

use crate::tests::helpers::{
    generate_address, Da, TestSequencer, GENESIS_SEQUENCER_DA_ADDRESS, GENESIS_SEQUENCER_KEY,
    INITIAL_BALANCE, INITIAL_BALANCE_LARGE, LOCKED_AMOUNT, REWARD_SEQUENCER_KEY, S,
    UNKNOWN_SEQUENCER_DA_ADDRESS,
};
use crate::{CallMessage, SequencerOutcome, SequencerRegistry};

/// Tests the slashing mechanism on the `end_batch_hook` method.
#[test]
fn end_batch_hook_slash() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    let balance_after_genesis = test_sequencer
        .query_sequencer_balance(&mut working_set)
        .unwrap();

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };

    let mut state_checkpoint = working_set.checkpoint().0;
    test_sequencer
        .registry
        .begin_batch_hook(
            &mut test_batch,
            &genesis_sequencer_da_address,
            &mut state_checkpoint,
        )
        .unwrap();

    let result = SequencerOutcome::Slashed;
    <SequencerRegistry<S, Da> as ApplyBatchHooks<MockDaSpec>>::end_batch_hook(
        &test_sequencer.registry,
        result,
        &genesis_sequencer_da_address,
        &mut state_checkpoint,
    );

    let resp = test_sequencer
        .query_sequencer_balance(&mut state_checkpoint)
        .unwrap();
    assert_eq!(balance_after_genesis, resp);
    let resp = test_sequencer
        .registry
        .resolve_da_address(&genesis_sequencer_da_address, &mut state_checkpoint);
    assert!(resp.is_none());
}

/// Tests the slashing mechanism for a preferred sequencer on the `end_batch_hook`
#[test]
fn end_batch_hook_slash_preferred_sequencer() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, true);
    let balance_after_genesis = test_sequencer
        .query_sequencer_balance(&mut working_set)
        .unwrap();

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };

    let mut state_checkpoint = working_set.checkpoint().0;

    test_sequencer
        .registry
        .begin_batch_hook(
            &mut test_batch,
            &genesis_sequencer_da_address,
            &mut state_checkpoint,
        )
        .unwrap();

    <SequencerRegistry<S, Da> as ApplyBatchHooks<MockDaSpec>>::end_batch_hook(
        &test_sequencer.registry,
        SequencerOutcome::Slashed,
        &genesis_sequencer_da_address,
        &mut state_checkpoint,
    );

    let working_set = &mut state_checkpoint.to_revertable_unmetered();
    let resp = test_sequencer.query_sequencer_balance(working_set).unwrap();
    assert_eq!(balance_after_genesis, resp);
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
fn end_batch_hook_slash_unknown_sequencer() {
    let (test_sequencer, working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };
    let sequencer_address = MockAddress::from(UNKNOWN_SEQUENCER_DA_ADDRESS);
    let mut state_checkpoint = working_set.checkpoint().0;
    test_sequencer
        .registry
        .begin_batch_hook(
            &mut test_batch,
            &MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS),
            &mut state_checkpoint,
        )
        .unwrap();

    let mut working_set = state_checkpoint.to_revertable_unmetered();

    let resp = test_sequencer
        .registry
        .sequencer_address(sequencer_address, &mut working_set)
        .unwrap();
    assert!(resp.address.is_none());

    let mut state_checkpoint = working_set.checkpoint().0;
    <SequencerRegistry<S, Da> as ApplyBatchHooks<MockDaSpec>>::end_batch_hook(
        &test_sequencer.registry,
        SequencerOutcome::Slashed,
        &MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS),
        &mut state_checkpoint,
    );

    let mut working_set = state_checkpoint.to_revertable_unmetered();
    let resp = test_sequencer
        .registry
        .sequencer_address(sequencer_address, &mut working_set)
        .unwrap();
    assert!(resp.address.is_none());
}

#[test]
fn begin_batch_hook_without_enough_stake() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    let mut test_blob = BatchWithId {
        txs: vec![],
        id: [0; 32],
    };

    test_sequencer.set_coins_amount_to_lock(LOCKED_AMOUNT + 1, &mut working_set);

    let mut state_checkpoint = working_set.checkpoint().0;
    let res = test_sequencer.registry.begin_batch_hook(
        &mut test_blob,
        &genesis_sequencer_da_address,
        &mut state_checkpoint,
    );

    assert!(
        res.is_err(),
        "the staked required amount was increased; the genesis sequencer is out of balance"
    );
}

#[test]
fn slashed_sequencer_should_not_preserve_balance() {
    let (test_sequencer, mut working_set) =
        TestSequencer::initialize_test(INITIAL_BALANCE_LARGE, false);

    // created settings

    let initial_balance = test_sequencer
        .bank_config
        .gas_token_config
        .address_and_balances[0]
        .1;
    let deposit_amount = 100;
    let stake_amount = test_sequencer.sequencer_config.minimum_bond;
    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    // sanity check the balance

    let genesis_sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);
    let balance_after_genesis = initial_balance - stake_amount;
    let balance = test_sequencer
        .bank
        .balance_of(
            None,
            genesis_sequencer_address,
            GAS_TOKEN_ID,
            &mut working_set,
        )
        .unwrap()
        .amount
        .unwrap();
    assert_eq!(balance, balance_after_genesis);

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, &mut working_set)
        .unwrap();
    assert_eq!(staked_balance, stake_amount);

    // deposit some additional stake amount

    let da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(genesis_sequencer_address, reward_address, 1);
    let deposit_message = CallMessage::Deposit {
        da_address: da_address.as_ref().to_vec(),
        amount: deposit_amount,
    };

    test_sequencer
        .registry
        .call(deposit_message, &sender_context, &mut working_set)
        .expect("Sequencer deposit has failed");

    let balance_after_deposit = balance_after_genesis - deposit_amount;
    let balance = test_sequencer
        .bank
        .balance_of(
            None,
            genesis_sequencer_address,
            GAS_TOKEN_ID,
            &mut working_set,
        )
        .unwrap()
        .amount
        .unwrap();
    assert_eq!(balance, balance_after_deposit);

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, &mut working_set)
        .unwrap();

    assert_eq!(
        staked_balance,
        stake_amount + deposit_amount,
        "the deposit should be added to the staked amount"
    );

    // submit an invalid block and expect the sequencer to be slashed

    assert!(test_sequencer
        .query_if_sequencer_is_allowed(&genesis_sequencer_da_address, &mut working_set),);

    let hash = [0u8; 32]; // invalid
    let result = SequencerOutcome::Slashed;

    let mut test_blob = BatchWithId {
        txs: vec![],
        id: hash,
    };

    let mut state_checkpoint = working_set.checkpoint().0;
    test_sequencer
        .registry
        .begin_batch_hook(
            &mut test_blob,
            &genesis_sequencer_da_address,
            &mut state_checkpoint,
        )
        .unwrap();

    test_sequencer.registry.end_batch_hook(
        result,
        &genesis_sequencer_da_address,
        &mut state_checkpoint,
    );
    let mut working_set = state_checkpoint.to_revertable_unmetered();

    assert!(
        !test_sequencer
            .query_if_sequencer_is_allowed(&genesis_sequencer_da_address, &mut working_set),
        "the sequencer was slashed and shouldn't be allowed"
    );

    let balance = test_sequencer
        .bank
        .balance_of(
            None,
            genesis_sequencer_address,
            GAS_TOKEN_ID,
            &mut working_set,
        )
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
        .get_sender_balance(&genesis_sequencer_da_address, &mut working_set);
    assert!(staked_balance.is_none());

    // register the sequencer again and check the balances

    let register_message = CallMessage::Register {
        da_address: genesis_sequencer_da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };

    test_sequencer
        .registry
        .call(register_message, &sender_context, &mut working_set)
        .expect("Sequencer registration has failed");

    let balance_after_re_register = balance_after_deposit - stake_amount;
    let balance = test_sequencer
        .bank
        .balance_of(
            None,
            genesis_sequencer_address,
            GAS_TOKEN_ID,
            &mut working_set,
        )
        .unwrap()
        .amount
        .unwrap();

    assert_eq!(
        balance, balance_after_re_register,
        "the stake amount should be deducted from the sender account"
    );

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, &mut working_set)
        .unwrap();

    assert_eq!(
        staked_balance, stake_amount,
        "the previous deposit should have been removed when the sequencer was slashed"
    );
}
