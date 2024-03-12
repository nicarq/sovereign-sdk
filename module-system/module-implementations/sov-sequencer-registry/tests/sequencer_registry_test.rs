use helpers::*;
use sov_mock_da::MockAddress;
use sov_modules_api::{Context, Error, Module, ModuleInfo, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;
use sov_sequencer_registry::{CallMessage, SequencerRegistry};

mod helpers;
type S = sov_test_utils::TestSpec;

// Happy path for registration and exit
// This test checks:
//  - genesis sequencer is present after genesis
//  - registration works, and funds are deducted
//  - exit works and funds are returned
#[test]
fn test_registration_lifecycle() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    // Check genesis
    {
        let sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);
        let registry_response = test_sequencer
            .registry
            .sequencer_address(MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS), working_set)
            .unwrap();
        assert_eq!(Some(sequencer_address), registry_response.address);
    }

    // Check normal lifecycle

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(ANOTHER_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(sequencer_address, reward_address, 1);

    let balance_before = test_sequencer
        .query_balance(sequencer_address, working_set)
        .unwrap()
        .amount
        .unwrap();

    let registry_response_before = test_sequencer
        .registry
        .sequencer_address(da_address, working_set)
        .unwrap();
    assert!(registry_response_before.address.is_none());

    let register_message = CallMessage::Register {
        da_address: da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };
    test_sequencer
        .registry
        .call(register_message, &sender_context, working_set)
        .expect("Sequencer registration has failed");

    let balance_after_registration = test_sequencer
        .query_balance(sequencer_address, working_set)
        .unwrap()
        .amount
        .unwrap();
    assert_eq!(balance_before - LOCKED_AMOUNT, balance_after_registration);

    let registry_response_after_registration = test_sequencer
        .registry
        .sequencer_address(da_address, working_set)
        .unwrap();
    assert_eq!(
        Some(sequencer_address),
        registry_response_after_registration.address
    );

    let exit_message = CallMessage::Exit {
        da_address: da_address.as_ref().to_vec(),
    };
    test_sequencer
        .registry
        .call(exit_message, &sender_context, working_set)
        .expect("Sequencer exit has failed");

    let balance_after_exit = test_sequencer
        .query_balance(sequencer_address, working_set)
        .unwrap()
        .amount
        .unwrap();
    assert_eq!(balance_before, balance_after_exit);

    let registry_response_after_exit = test_sequencer
        .registry
        .sequencer_address(da_address, working_set)
        .unwrap();
    assert!(registry_response_after_exit.address.is_none());
}

#[test]
fn test_registration_not_enough_funds() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(LOW_FUND_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(sequencer_address, reward_address, 1);

    let register_message = CallMessage::Register {
        da_address: da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };
    let response = test_sequencer
        .registry
        .call(register_message, &sender_context, working_set);

    assert!(
        response.is_err(),
        "insufficient funds registration should fail"
    );
    let Error::ModuleError(err) = response.err().unwrap();
    let mut chain = err.chain();
    let message_1 = chain.next().unwrap().to_string();
    let message_2 = chain.next().unwrap().to_string();
    let message_3 = chain.next().unwrap().to_string();
    assert!(chain.next().is_none());

    assert_eq!(
        format!(
            "Failed transfer from={} to={} of coins(token_address={} amount={})",
            sequencer_address,
            test_sequencer.registry.address(),
            test_sequencer.sequencer_config.coins_to_lock.token_address,
            LOCKED_AMOUNT,
        ),
        message_1
    );
    assert_eq!(
        format!(
            "Incorrect balance on={} for token={}",
            sequencer_address, GENESIS_TOKEN_NAME,
        ),
        message_2,
    );
    assert_eq!(
        format!("Insufficient funds for {}", sequencer_address),
        message_3,
    );
}

#[test]
fn test_registration_second_time() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    let da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(sequencer_address, reward_address, 1);

    let register_message = CallMessage::Register {
        da_address: da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };
    let response = test_sequencer
        .registry
        .call(register_message, &sender_context, working_set);

    assert!(response.is_err(), "duplicate registration should fail");
    let expected_error_message = format!("sequencer {} already registered", sequencer_address);
    let actual_error_message = response.err().unwrap().to_string();

    assert_eq!(expected_error_message, actual_error_message);
}

#[test]
fn test_exit_different_sender() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    let sequencer_address = generate_address(ANOTHER_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(sequencer_address, reward_address, 1);
    let attacker_address = generate_address("some_random_key");
    let attacker_context = Context::<S>::new(attacker_address, reward_address, 1);

    let register_message = CallMessage::Register {
        da_address: ANOTHER_SEQUENCER_DA_ADDRESS.to_vec(),
        amount: LOCKED_AMOUNT,
    };
    test_sequencer
        .registry
        .call(register_message, &sender_context, working_set)
        .expect("Sequencer registration has failed");

    let exit_message = CallMessage::Exit {
        da_address: ANOTHER_SEQUENCER_DA_ADDRESS.to_vec(),
    };
    let response = test_sequencer
        .registry
        .call(exit_message, &attacker_context, working_set);

    assert!(
        response.is_err(),
        "exit by non authorized sender should fail"
    );
    let actual_error_message = response.err().unwrap().to_string();

    assert_eq!(
        format!(
            "Unauthorized exit attempt from sequencer `{}`",
            attacker_address,
        ),
        actual_error_message
    );
}

#[test]
fn test_allow_exit_last_sequencer() {
    let test_sequencer = create_test_sequencer();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    let sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);
    let rewards_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(sequencer_address, rewards_address, 1);
    let exit_message = CallMessage::Exit {
        da_address: GENESIS_SEQUENCER_DA_ADDRESS.to_vec(),
    };
    test_sequencer
        .registry
        .call(exit_message, &sender_context, working_set)
        .expect("Last sequencer exit has failed");
}

#[test]
fn test_preferred_sequencer_returned_and_removed() {
    let bank = sov_bank::Bank::<S>::default();
    let (bank_config, seq_rollup_address) = create_bank_config();

    let token_address = bank_config.tokens[0].token_address;

    let registry = SequencerRegistry::<S, Da>::default();
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

    assert_eq!(
        Some(test_sequencer.sequencer_config.seq_da_address),
        test_sequencer.registry.get_preferred_sequencer(working_set)
    );

    let sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(sequencer_address, reward_address, 1);
    let exit_message = CallMessage::Exit {
        da_address: GENESIS_SEQUENCER_DA_ADDRESS.to_vec(),
    };
    test_sequencer
        .registry
        .call(exit_message, &sender_context, working_set)
        .expect("Last sequencer exit has failed");

    // Preferred sequencer exited, so the result is none
    assert!(test_sequencer
        .registry
        .get_preferred_sequencer(working_set)
        .is_none());
}

#[test]
fn test_registration_balance_increase() {
    let test_sequencer = create_test_sequencer_large_balance();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    // created settings

    let initial_balance = test_sequencer.bank_config.tokens[0].address_and_balances[0].1;
    let stake_amount = test_sequencer.sequencer_config.coins_to_lock.amount;
    let stake_increase = 1;

    // Register a sequencer

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(ANOTHER_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(sequencer_address, reward_address, 1);

    let register_message = CallMessage::Register {
        da_address: da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };

    test_sequencer
        .registry
        .call(register_message, &sender_context, working_set)
        .expect("Sequencer registration has failed");

    // Sanity check

    let balance_after_registration = test_sequencer
        .query_balance(sequencer_address, working_set)
        .unwrap()
        .amount
        .unwrap();
    assert_eq!(initial_balance - stake_amount, balance_after_registration);

    // Assert the registry balance is the staked amount

    let sender_balance = test_sequencer
        .query_sender_balance(&da_address, working_set)
        .unwrap();
    assert_eq!(stake_amount, sender_balance);

    // Sanity check the sequencer allowed status

    assert!(test_sequencer.query_if_sequencer_is_allowed(&da_address, working_set));

    // Increases the stake value and expects the sequencer to preserve his balance and no longer be
    // allowed

    let stake_amount = stake_amount + stake_increase;
    test_sequencer
        .set_coins_amount_to_lock(stake_amount, working_set)
        .unwrap();

    let balance_after_update = test_sequencer
        .query_balance(sequencer_address, working_set)
        .unwrap()
        .amount
        .unwrap();

    assert_eq!(balance_after_registration, balance_after_update);
    assert!(!test_sequencer.query_if_sequencer_is_allowed(&da_address, working_set));

    // Increase the balance of the sequencer and assert sequencer is allowed

    let deposit_message = CallMessage::Deposit {
        da_address: da_address.as_ref().to_vec(),
        amount: stake_increase,
    };

    test_sequencer
        .registry
        .call(deposit_message, &sender_context, working_set)
        .expect("Sequencer deposit has failed");

    let new_sender_balance = test_sequencer
        .query_sender_balance(&da_address, working_set)
        .unwrap();

    assert_eq!(sender_balance + stake_increase, new_sender_balance);
    assert!(test_sequencer.query_if_sequencer_is_allowed(&da_address, working_set));
}

#[test]
fn test_balance_increase_fails_if_insufficient_funds() {
    let test_sequencer = create_test_sequencer_large_balance();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    // created settings

    let initial_balance = test_sequencer.bank_config.tokens[0].address_and_balances[0].1;
    let stake_amount = test_sequencer.sequencer_config.coins_to_lock.amount;
    let stake_increase = initial_balance;

    // Register a sequencer

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(ANOTHER_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(sequencer_address, reward_address, 1);

    let register_message = CallMessage::Register {
        da_address: da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };

    test_sequencer
        .registry
        .call(register_message, &sender_context, working_set)
        .expect("Sequencer registration has failed");

    // Sanity check the sequencer allowed status

    assert!(test_sequencer.query_if_sequencer_is_allowed(&da_address, working_set));

    // Increase the stake value and assert the sequencer is no longer allowed

    let stake_amount = stake_amount + stake_increase;
    test_sequencer
        .set_coins_amount_to_lock(stake_amount, working_set)
        .unwrap();
    assert!(!test_sequencer.query_if_sequencer_is_allowed(&da_address, working_set));

    // Attempt to deposit the required amount and expect failure

    let deposit_message = CallMessage::Deposit {
        da_address: da_address.as_ref().to_vec(),
        amount: stake_increase,
    };

    let res = test_sequencer
        .registry
        .call(deposit_message, &sender_context, working_set);

    assert!(res.is_err());
    assert!(!test_sequencer.query_if_sequencer_is_allowed(&da_address, working_set));
}

#[test]
fn test_non_registered_sequencer_is_not_allowed() {
    let test_sequencer = create_test_sequencer_large_balance();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);

    assert!(!test_sequencer.query_if_sequencer_is_allowed(&da_address, working_set));
}

#[test]
fn test_balance_increase_fails_for_unknown_sequencer() {
    let test_sequencer = create_test_sequencer_large_balance();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    test_sequencer.genesis(working_set);

    // created settings

    let stake_amount = test_sequencer.sequencer_config.coins_to_lock.amount;

    // Register a sequencer

    let da_address = MockAddress::from(ANOTHER_SEQUENCER_DA_ADDRESS);
    let unknown_address = MockAddress::from(UNKNOWN_SEQUENCER_DA_ADDRESS);

    let sequencer_address = generate_address(ANOTHER_SEQUENCER_KEY);
    let reward_address = generate_address(REWARD_SEQUENCER_KEY);
    let sender_context = Context::<S>::new(sequencer_address, reward_address, 1);

    let register_message = CallMessage::Register {
        da_address: da_address.as_ref().to_vec(),
        amount: LOCKED_AMOUNT,
    };

    test_sequencer
        .registry
        .call(register_message, &sender_context, working_set)
        .expect("Sequencer registration has failed");

    // Sanity check the sequencer allowed status

    assert!(!test_sequencer.query_if_sequencer_is_allowed(&unknown_address, working_set));

    // Attempt to deposit 1 coin into unknown sequencer

    let deposit_message = CallMessage::Deposit {
        da_address: unknown_address.as_ref().to_vec(),
        amount: 1,
    };

    let res = test_sequencer
        .registry
        .call(deposit_message, &sender_context, working_set);

    assert!(res.is_err());
    assert!(!test_sequencer.query_if_sequencer_is_allowed(&unknown_address, working_set));

    // Attempt to deposit stake amount into unknown sequencer

    let deposit_message = CallMessage::Deposit {
        da_address: unknown_address.as_ref().to_vec(),
        amount: stake_amount,
    };

    let res = test_sequencer
        .registry
        .call(deposit_message, &sender_context, working_set);

    assert!(res.is_err());
    assert!(!test_sequencer.query_if_sequencer_is_allowed(&unknown_address, working_set));
}
