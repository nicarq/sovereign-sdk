use std::convert::Infallible;

use sov_bank::GAS_TOKEN_ID;
use sov_mock_da::MockAddress;
use sov_modules_api::{Context, Module};
use sov_test_utils::{TEST_DEFAULT_USER_BALANCE, TEST_DEFAULT_USER_STAKE};

use crate::tests::helpers::{
    generate_address, TestSequencer, GENESIS_SEQUENCER_DA_ADDRESS, GENESIS_SEQUENCER_KEY,
    REWARD_SEQUENCER_KEY, S, UNKNOWN_SEQUENCER_DA_ADDRESS,
};
use crate::CallMessage;

/// Tests the slashing mechanism.
#[test]
fn test_slash_sequencer() -> Result<(), Infallible> {
    let (test_sequencer, mut state) =
        TestSequencer::initialize_test(TEST_DEFAULT_USER_BALANCE, false)?;

    let balance_after_genesis = test_sequencer.query_sequencer_balance(&mut state)?.unwrap();

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    test_sequencer
        .registry
        .is_sender_allowed(&genesis_sequencer_da_address, &mut state)
        .unwrap();

    test_sequencer
        .registry
        .slash_sequencer(&genesis_sequencer_da_address, &mut state);

    let resp = test_sequencer.query_sequencer_balance(&mut state)?.unwrap();
    assert_eq!(balance_after_genesis, resp);
    let resp = test_sequencer
        .registry
        .resolve_da_address(&genesis_sequencer_da_address, &mut state)?;
    assert!(resp.is_none());

    Ok(())
}

/// Tests the slashing mechanism for a preferred sequencer on the `end_batch_hook`
#[test]
fn test_slash_preferred_sequencer() -> Result<(), Infallible> {
    let (test_sequencer, mut state) =
        TestSequencer::initialize_test(TEST_DEFAULT_USER_BALANCE, true)?;
    let balance_after_genesis = test_sequencer.query_sequencer_balance(&mut state)?.unwrap();

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    test_sequencer
        .registry
        .is_sender_allowed(&genesis_sequencer_da_address, &mut state)
        .unwrap();

    test_sequencer
        .registry
        .slash_sequencer(&genesis_sequencer_da_address, &mut state);

    let resp = test_sequencer.query_sequencer_balance(&mut state)?.unwrap();
    assert_eq!(balance_after_genesis, resp);
    let resp = test_sequencer
        .registry
        .get_sequencer_address(MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS), &mut state)?;
    assert!(resp.is_none());

    assert!(test_sequencer
        .registry
        .get_preferred_sequencer(&mut state)?
        .is_none());

    Ok(())
}

#[test]
fn test_slash_unknown_sequencer() -> Result<(), Infallible> {
    let (test_sequencer, mut state) =
        TestSequencer::initialize_test(TEST_DEFAULT_USER_BALANCE, false)?;

    let sequencer_address = MockAddress::from(UNKNOWN_SEQUENCER_DA_ADDRESS);
    test_sequencer
        .registry
        .is_sender_allowed(&MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS), &mut state)
        .unwrap();

    let resp = test_sequencer
        .registry
        .get_sequencer_address(sequencer_address, &mut state)?;
    assert!(resp.is_none());

    test_sequencer
        .registry
        .slash_sequencer(&sequencer_address, &mut state);

    let resp = test_sequencer
        .registry
        .get_sequencer_address(sequencer_address, &mut state)?;
    assert!(resp.is_none());

    Ok(())
}

#[test]
fn test_sequencer_without_enough_stake() -> Result<(), Infallible> {
    let (test_sequencer, mut state) =
        TestSequencer::initialize_test(TEST_DEFAULT_USER_BALANCE, false)?;

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    test_sequencer.set_coins_amount_to_lock(TEST_DEFAULT_USER_STAKE + 1, &mut state)?;

    let res = test_sequencer
        .registry
        .is_sender_allowed(&genesis_sequencer_da_address, &mut state);

    assert!(
        res.is_err(),
        "the staked required amount was increased; the genesis sequencer is out of balance"
    );

    Ok(())
}

#[test]
fn slashed_sequencer_should_not_preserve_balance() -> Result<(), Infallible> {
    let (test_sequencer, mut state) =
        TestSequencer::initialize_test(TEST_DEFAULT_USER_BALANCE, false)?;

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
        .get_balance_of(&genesis_sequencer_address, GAS_TOKEN_ID, &mut state)?
        .unwrap();

    assert_eq!(balance, balance_after_genesis);

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, &mut state)?
        .unwrap();
    assert_eq!(staked_balance, stake_amount);

    // deposit some additional stake amount

    let da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(
        genesis_sequencer_address,
        Default::default(),
        reward_address,
        1,
    );
    let deposit_message = CallMessage::Deposit {
        da_address: da_address.as_ref().to_vec(),
        amount: deposit_amount,
    };

    let mut state = state.to_working_set_unmetered();
    test_sequencer
        .registry
        .call(deposit_message, &sender_context, &mut state)
        .expect("Sequencer deposit has failed");
    let mut state = state.checkpoint().0;

    let balance_after_deposit = balance_after_genesis - deposit_amount;
    let balance = test_sequencer
        .bank
        .get_balance_of(&genesis_sequencer_address, GAS_TOKEN_ID, &mut state)?
        .unwrap();

    assert_eq!(balance, balance_after_deposit);

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, &mut state)?
        .unwrap();

    assert_eq!(
        staked_balance,
        stake_amount + deposit_amount,
        "the deposit should be added to the staked amount"
    );

    // submit an invalid block and expect the sequencer to be slashed

    assert!(
        test_sequencer.query_if_sequencer_is_allowed(&genesis_sequencer_da_address, &mut state),
    );

    test_sequencer
        .registry
        .is_sender_allowed(&genesis_sequencer_da_address, &mut state)
        .unwrap();

    test_sequencer
        .registry
        .slash_sequencer(&genesis_sequencer_da_address, &mut state);

    assert!(
        !test_sequencer.query_if_sequencer_is_allowed(&genesis_sequencer_da_address, &mut state),
        "the sequencer was slashed and shouldn't be allowed"
    );

    let balance = test_sequencer
        .bank
        .get_balance_of(&genesis_sequencer_address, GAS_TOKEN_ID, &mut state)?
        .unwrap();

    assert_eq!(
        balance,
        balance_after_deposit,
        "the balance should be unchanged after slash; the slashed tokens are frozen on the registry account"
    );

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, &mut state)?;
    assert!(staked_balance.is_none());

    // register the sequencer again and check the balances

    let register_message = CallMessage::Register {
        da_address: genesis_sequencer_da_address.as_ref().to_vec(),
        amount: TEST_DEFAULT_USER_STAKE,
    };

    let mut state = state.to_working_set_unmetered();
    test_sequencer
        .registry
        .call(register_message, &sender_context, &mut state)
        .expect("Sequencer registration has failed");
    let mut state = state.checkpoint().0;

    let balance_after_re_register = balance_after_deposit - stake_amount;
    let balance = test_sequencer
        .bank
        .get_balance_of(&genesis_sequencer_address, GAS_TOKEN_ID, &mut state)?
        .unwrap();

    assert_eq!(
        balance, balance_after_re_register,
        "the stake amount should be deducted from the sender account"
    );

    let staked_balance = test_sequencer
        .registry
        .get_sender_balance(&genesis_sequencer_da_address, &mut state)?
        .unwrap();

    assert_eq!(
        staked_balance, stake_amount,
        "the previous deposit should have been removed when the sequencer was slashed"
    );

    Ok(())
}
