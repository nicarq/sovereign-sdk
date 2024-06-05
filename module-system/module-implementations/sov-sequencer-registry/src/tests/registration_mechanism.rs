use sov_bank::Payable;
use sov_mock_da::MockAddress;
use sov_modules_api::{Context, Module};

use crate::tests::helpers::{
    generate_address, TestSequencer, ANOTHER_SEQUENCER_DA_ADDRESS, ANOTHER_SEQUENCER_KEY,
    GENESIS_SEQUENCER_DA_ADDRESS, GENESIS_SEQUENCER_KEY, INITIAL_BALANCE, INITIAL_BALANCE_LARGE,
    LOCKED_AMOUNT, LOW_FUND_KEY, REWARD_SEQUENCER_KEY, UNKNOWN_SEQUENCER_DA_ADDRESS,
};
use crate::{CallMessage, SequencerRegistryError};

type S = sov_test_utils::TestSpec;

// Happy path for registration and exit
// This test checks:
//  - genesis sequencer is present after genesis
//  - registration works, and funds are deducted
//  - exit works and funds are returned
#[test]
fn test_registration_lifecycle() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    // Check normal lifecycle

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(ANOTHER_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context =
        Context::<S>::new(sequencer_address, Default::default(), reward_address, 1);

    let balance_before = test_sequencer
        .query_balance((&sequencer_address).as_token_holder(), &mut working_set)
        .unwrap();

    let registry_response_before = test_sequencer
        .registry
        .get_sequencer_address(da_address, &mut working_set);
    assert!(registry_response_before.is_none());

    let register_message = CallMessage::Register {
        da_address: da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };
    test_sequencer
        .registry
        .call(register_message, &sender_context, &mut working_set)
        .expect("Sequencer registration has failed");

    let balance_after_registration = test_sequencer
        .query_balance((&sequencer_address).as_token_holder(), &mut working_set)
        .unwrap();
    assert_eq!(balance_before - LOCKED_AMOUNT, balance_after_registration);

    let registry_response_after_registration = test_sequencer
        .registry
        .get_sequencer_address(da_address, &mut working_set);
    assert_eq!(
        Some(sequencer_address),
        registry_response_after_registration
    );

    let exit_message = CallMessage::Exit {
        da_address: da_address.as_ref().to_vec(),
    };
    test_sequencer
        .registry
        .call(exit_message, &sender_context, &mut working_set)
        .expect("Sequencer exit has failed");

    let balance_after_exit = test_sequencer
        .query_balance((&sequencer_address).as_token_holder(), &mut working_set)
        .unwrap();
    assert_eq!(balance_before, balance_after_exit);

    let registry_response_after_exit = test_sequencer
        .registry
        .get_sequencer_address(da_address, &mut working_set);
    assert!(registry_response_after_exit.is_none());
}

#[test]
fn test_registration_not_enough_funds() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(LOW_FUND_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context =
        Context::<S>::new(sequencer_address, Default::default(), reward_address, 1);

    let response = test_sequencer.registry.register(
        &da_address,
        LOCKED_AMOUNT,
        &sender_context,
        &mut working_set,
    );

    // Note: the next PR will add a check for the error message back
    assert!(
        response.is_err(),
        "insufficient funds registration should fail"
    );

    assert_eq!(
        response.unwrap_err(),
        SequencerRegistryError::InsufficientFundsToRegister(LOCKED_AMOUNT)
    );
}

#[test]
fn test_registration_second_time() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    let da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context =
        Context::<S>::new(sequencer_address, Default::default(), reward_address, 1);

    let response = test_sequencer.registry.register(
        &da_address,
        LOCKED_AMOUNT,
        &sender_context,
        &mut working_set,
    );

    // Note: the next PR will add a check for the error message back
    assert!(response.is_err(), "duplicate registration should fail");

    assert_eq!(
        response.unwrap_err(),
        SequencerRegistryError::SequencerAlreadyRegistered(sequencer_address)
    );
}

#[test]
fn test_exit_different_sender() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    let sequencer_address = generate_address(ANOTHER_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context =
        Context::<S>::new(sequencer_address, Default::default(), reward_address, 1);
    let attacker_address = generate_address("some_random_key");
    let attacker_context =
        Context::<S>::new(attacker_address, Default::default(), reward_address, 1);

    test_sequencer
        .registry
        .register(
            &MockAddress::new(ANOTHER_SEQUENCER_DA_ADDRESS),
            LOCKED_AMOUNT,
            &sender_context,
            &mut working_set,
        )
        .expect("Sequencer registration has failed");

    let response = test_sequencer.registry.exit(
        &MockAddress::new(ANOTHER_SEQUENCER_DA_ADDRESS),
        &attacker_context,
        &mut working_set,
    );

    // Note: the next PR will add a check for the error message back
    assert!(
        response.is_err(),
        "exit by non authorized sender should fail"
    );

    assert_eq!(
        response.unwrap_err(),
        SequencerRegistryError::SuppliedAddressDoesNotMatchTxSender {
            parameter: sequencer_address,
            sender: attacker_address,
        }
    );
}

#[test]
fn test_allow_exit_last_sequencer() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    let sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);
    let rewards_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context =
        Context::<S>::new(sequencer_address, Default::default(), rewards_address, 1);
    let exit_message = CallMessage::Exit {
        da_address: GENESIS_SEQUENCER_DA_ADDRESS.to_vec(),
    };
    test_sequencer
        .registry
        .call(exit_message, &sender_context, &mut working_set)
        .expect("Last sequencer exit has failed");
}

/// This regression test ensures that a sequencer cannot exit during processing
/// of a batch that they've submitted.
#[test]
fn test_prevent_exit_during_own_batch() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    let sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);
    let sender_context =
        Context::<S>::new(sequencer_address, Default::default(), sequencer_address, 1);
    let exit_message = CallMessage::Exit {
        da_address: GENESIS_SEQUENCER_DA_ADDRESS.to_vec(),
    };
    assert!(test_sequencer
        .registry
        .call(exit_message, &sender_context, &mut working_set)
        .is_err());
}

#[test]
fn test_preferred_sequencer_returned_and_removed() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, true);

    assert_eq!(
        Some(test_sequencer.sequencer_config.seq_da_address),
        test_sequencer
            .registry
            .get_preferred_sequencer(&mut working_set)
    );

    let sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context =
        Context::<S>::new(sequencer_address, Default::default(), reward_address, 1);
    let exit_message = CallMessage::Exit {
        da_address: GENESIS_SEQUENCER_DA_ADDRESS.to_vec(),
    };
    test_sequencer
        .registry
        .call(exit_message, &sender_context, &mut working_set)
        .expect("Last sequencer exit has failed");

    // Preferred sequencer exited, so the result is none
    assert!(test_sequencer
        .registry
        .get_preferred_sequencer(&mut working_set)
        .is_none());
}

#[test]
fn test_registration_balance_increase() {
    let (test_sequencer, mut working_set) =
        TestSequencer::initialize_test(INITIAL_BALANCE_LARGE, false);

    // created settings

    let initial_balance = test_sequencer
        .bank_config
        .gas_token_config
        .address_and_balances[0]
        .1;
    let stake_amount = test_sequencer.sequencer_config.minimum_bond;
    let stake_increase = 1;

    // Register a sequencer

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(ANOTHER_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context =
        Context::<S>::new(sequencer_address, Default::default(), reward_address, 1);

    let register_message = CallMessage::Register {
        da_address: da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };

    test_sequencer
        .registry
        .call(register_message, &sender_context, &mut working_set)
        .expect("Sequencer registration has failed");

    // Sanity check

    let balance_after_registration = test_sequencer
        .query_balance((&sequencer_address).as_token_holder(), &mut working_set)
        .unwrap();
    assert_eq!(initial_balance - stake_amount, balance_after_registration);

    // Assert the registry balance is the staked amount

    let sender_balance = test_sequencer
        .query_sender_balance(&da_address, &mut working_set)
        .unwrap();
    assert_eq!(stake_amount, sender_balance);

    // Sanity check the sequencer allowed status

    assert!(test_sequencer.query_if_sequencer_is_allowed(&da_address, &mut working_set));

    // Increases the stake value and expects the sequencer to preserve his balance and no longer be
    // allowed

    let stake_amount = stake_amount + stake_increase;
    test_sequencer.set_coins_amount_to_lock(stake_amount, &mut working_set);

    let balance_after_update = test_sequencer
        .query_balance((&sequencer_address).as_token_holder(), &mut working_set)
        .unwrap();

    assert_eq!(balance_after_registration, balance_after_update);
    assert!(!test_sequencer.query_if_sequencer_is_allowed(&da_address, &mut working_set));

    // Increase the balance of the sequencer and assert sequencer is allowed

    let deposit_message = CallMessage::Deposit {
        da_address: da_address.as_ref().to_vec(),
        amount: stake_increase,
    };

    test_sequencer
        .registry
        .call(deposit_message, &sender_context, &mut working_set)
        .expect("Sequencer deposit has failed");

    let new_sender_balance = test_sequencer
        .query_sender_balance(&da_address, &mut working_set)
        .unwrap();

    assert_eq!(sender_balance + stake_increase, new_sender_balance);
    assert!(test_sequencer.query_if_sequencer_is_allowed(&da_address, &mut working_set));
}

#[test]
fn test_balance_increase_fails_if_insufficient_funds() {
    let (test_sequencer, mut working_set) =
        TestSequencer::initialize_test(INITIAL_BALANCE_LARGE, false);

    // created settings

    let initial_balance = test_sequencer
        .bank_config
        .gas_token_config
        .address_and_balances[0]
        .1;
    let stake_amount = test_sequencer.sequencer_config.minimum_bond;
    let stake_increase = initial_balance;

    // Register a sequencer

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(ANOTHER_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context =
        Context::<S>::new(sequencer_address, Default::default(), reward_address, 1);

    let register_message = CallMessage::Register {
        da_address: da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };

    test_sequencer
        .registry
        .call(register_message, &sender_context, &mut working_set)
        .expect("Sequencer registration has failed");

    // Sanity check the sequencer allowed status

    assert!(test_sequencer.query_if_sequencer_is_allowed(&da_address, &mut working_set));

    // Increase the stake value and assert the sequencer is no longer allowed

    let stake_amount = stake_amount + stake_increase;
    test_sequencer.set_coins_amount_to_lock(stake_amount, &mut working_set);
    assert!(!test_sequencer.query_if_sequencer_is_allowed(&da_address, &mut working_set));

    // Attempt to deposit the required amount and expect failure

    let deposit_message = CallMessage::Deposit {
        da_address: da_address.as_ref().to_vec(),
        amount: stake_increase,
    };

    let res = test_sequencer
        .registry
        .call(deposit_message, &sender_context, &mut working_set);

    assert!(res.is_err());
    assert!(!test_sequencer.query_if_sequencer_is_allowed(&da_address, &mut working_set));
}

#[test]
fn test_non_registered_sequencer_is_not_allowed() {
    let (test_sequencer, mut working_set) =
        TestSequencer::initialize_test(INITIAL_BALANCE_LARGE, false);

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);

    assert!(!test_sequencer.query_if_sequencer_is_allowed(&da_address, &mut working_set));
}

#[test]
fn test_balance_increase_fails_for_unknown_sequencer() {
    let (test_sequencer, mut working_set) =
        TestSequencer::initialize_test(INITIAL_BALANCE_LARGE, false);

    // created settings

    let stake_amount = test_sequencer.sequencer_config.minimum_bond;

    // Register a sequencer

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);
    let unknown_address = MockAddress::from(UNKNOWN_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(ANOTHER_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context =
        Context::<S>::new(sequencer_address, Default::default(), reward_address, 1);

    let register_message = CallMessage::Register {
        da_address: da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };

    test_sequencer
        .registry
        .call(register_message, &sender_context, &mut working_set)
        .expect("Sequencer registration has failed");

    // Sanity check the sequencer allowed status

    assert!(!test_sequencer.query_if_sequencer_is_allowed(&unknown_address, &mut working_set));

    // Attempt to deposit 1 coin into unknown sequencer

    let deposit_message = CallMessage::Deposit {
        da_address: unknown_address.as_ref().to_vec(),
        amount: 1,
    };

    let res = test_sequencer
        .registry
        .call(deposit_message, &sender_context, &mut working_set);

    assert!(res.is_err());
    assert!(!test_sequencer.query_if_sequencer_is_allowed(&unknown_address, &mut working_set));

    // Attempt to deposit stake amount into unknown sequencer

    let deposit_message = CallMessage::Deposit {
        da_address: unknown_address.as_ref().to_vec(),
        amount: stake_amount,
    };

    let res = test_sequencer
        .registry
        .call(deposit_message, &sender_context, &mut working_set);

    assert!(res.is_err());
    assert!(!test_sequencer.query_if_sequencer_is_allowed(&unknown_address, &mut working_set));
}
