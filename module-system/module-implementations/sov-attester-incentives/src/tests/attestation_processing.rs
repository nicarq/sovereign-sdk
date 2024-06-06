use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_chain_state::ChainState;
use sov_mock_da::MockDaSpec;
use sov_modules_api::hooks::TxHooks;
use sov_modules_api::optimistic::Attestation;
use sov_modules_api::{Context, CryptoSpec, GasMeter, Module, PrivateKey, Spec, WorkingSet};
use sov_prover_storage_manager::SimpleStorageManager;
use sov_state::{Storage, StorageProof};
use sov_test_utils::generate_optimistic_runtime;
use sov_test_utils::runtime::genesis::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::sov_attester_incentives::{
    AttesterIncentives, CallMessage, Event, Role, WrappedAttestation,
};
use sov_test_utils::runtime::{
    run_test_with_setup_fn, MessageType, SlotTestCase, TxOutcome, TxTestCase,
};

use crate::call::AttesterIncentiveErrors;
use crate::tests::helpers::{
    commit_get_new_storage, setup, ExecutionSimulationVars, BOND_AMOUNT, INIT_HEIGHT,
};
type S = sov_test_utils::TestSpec;
const DUMMY_CALL_MESSAGE: CallMessage<S, MockDaSpec> = CallMessage::UnbondChallenger; // This will get overwritten by the setup hook

#[allow(clippy::type_complexity)]
fn create_attestation(
    slot_to_attest: u64,
    attester_address: &<S as Spec>::Address,
    state: &mut WorkingSet<S>,
) -> Attestation<
    MockDaSpec,
    StorageProof<<<S as Spec>::Storage as Storage>::Proof>,
    <<S as Spec>::Storage as Storage>::Root,
> {
    let chain_state = ChainState::<S, MockDaSpec>::default();

    // Get the values for the transition being attested
    let current_transition = chain_state
        .get_historical_transitions(slot_to_attest, state)
        .unwrap();

    let prev_root = if slot_to_attest == 1 {
        chain_state.get_genesis_hash(state)
    } else {
        chain_state
            .get_historical_transitions(slot_to_attest - 1, state)
            .map(|t| *t.post_state_root())
    }
    .unwrap();

    // Recall that the attester must be bonded *at a height that light clients consider finalized*
    // Since finalization takes a long time (24 hours), we know that it will never advance past slot 1 in tests.
    let proof_of_bond = AttesterIncentives::<S, MockDaSpec>::default()
        .bonded_attesters
        .get_with_proof(attester_address, &mut state.get_archival_at(1));

    Attestation {
        initial_state_root: prev_root,
        slot_hash: *current_transition.slot_hash(),
        post_state_root: *current_transition.post_state_root(),
        proof_of_bond: sov_modules_api::optimistic::ProofOfBond {
            claimed_transition_num: slot_to_attest,
            proof: proof_of_bond,
        },
    }
}

/// Start by testing the positive case where the attestations are valid. We check that...
/// valid attestations are processed correctly
/// attesters are rewarded as expected
#[test]
fn test_process_valid_attestation() {
    generate_optimistic_runtime!(AttesterRuntime <=);

    // Generate a genesis config, then overwrite the attester key/address with ones that
    // we know. We leave the other values untouched.
    let mut genesis_config = HighLevelOptimisticGenesisConfig::generate();
    let attester = &mut genesis_config.initial_attester;
    let attester_key = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate();
    attester.address = <S as Spec>::Address::from(&attester_key.pub_key());
    let attester_address = attester.address;
    let initial_balance = attester.additional_balance.unwrap_or_default();
    let expected_balance = initial_balance;

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into());
    let mut last_attested_slot = 0;

    // Create a transaction setup function which overwrites dummy CallMessages with a valid attestation.
    // We have to use the test frawework's setup hook to do this, since we don't know the correct state
    // roots in advance.
    let mut attestation_setup =
        move |message: &mut <AttesterIncentives<S, MockDaSpec> as Module>::CallMessage,
              _root: <<S as Spec>::Storage as Storage>::Root,
              state: &mut <AttesterRuntime<S, MockDaSpec> as TxHooks>::TxState| {
            if message == &DUMMY_CALL_MESSAGE {
                let next_slot = last_attested_slot + 1;
                let attestation = create_attestation(next_slot, &attester_address, state);
                *message =
                    CallMessage::ProcessAttestation(WrappedAttestation { inner: attestation });
                last_attested_slot = next_slot;
            }
        };

    // We use an arc of an atomic to do accounting for the expected balance.
    // because of limitations in rusts capture rules, we need a bunch of clones
    // of this arc ahead of time
    let expected_balance = Arc::new(AtomicU64::new(expected_balance));
    let expected_balance_ref_1 = expected_balance.clone();
    let expected_balance_ref_2 = expected_balance.clone();
    let expected_balance_ref_3 = expected_balance.clone();

    // We run a test with 5 slots (plus genesis). The first two slots are empty; using our simple
    // setup function, we can only attest to slots that have at least on extra slot on top. (In other
    // words, attestations lag by two slots). Then we run three attestations, one for each of the
    // empty blocks and one for the first slot that contains a transaction. This allows us to test
    // that gas metering is done correctly.
    run_test_with_setup_fn(
        genesis.into_genesis_params(),
        &mut attestation_setup,
        vec![
            // Run any empty slot, and check that the attester has the correct bond amount from genesis
            SlotTestCase::<_, AttesterIncentives<S, MockDaSpec>, _> {
                transaction_test_cases: vec![],
                post_hook: Box::new(move |ws| {
                    assert_eq!(
                        AttesterIncentives::<S, MockDaSpec>::default()
                            .bonded_attesters
                            .get(&attester_address, ws)
                            .unwrap_or_default(),
                        initial_balance,
                    );
                }),
            },
            // Run an empty slot
            SlotTestCase::empty(),
            // Attest to the first slot. Check that a ProcessedValidAttestation attestation
            // event is emitted and do necessary accounting to check the attester's balance later
            SlotTestCase {
                transaction_test_cases: vec![TxTestCase {
                    outcome: TxOutcome::Applied(Box::new(move |ws: &mut WorkingSet<S>| {
                        // Do accounting for the attester's balance
                        {
                            // The attester's balance should be decremented by the gas used
                            expected_balance.fetch_sub(
                                ws.gas_used_value(),
                                std::sync::atomic::Ordering::SeqCst,
                            );
                            // We know that attester will attest to this slot later, so he'll get back some of his gas at that point.
                            expected_balance.fetch_add(
                                AttesterIncentives::<S, MockDaSpec>::default()
                                    .burn_rate()
                                    .apply(ws.gas_used_value()),
                                std::sync::atomic::Ordering::SeqCst,
                            );
                        }

                        // Check that the attestation succeeded
                        assert!(ws.events().iter().any(|event| matches!(
                            event.downcast_ref::<Event<S>>(),
                            Some(Event::ProcessedValidAttestation { .. })
                        )));
                    })),
                    message: MessageType::Plain(DUMMY_CALL_MESSAGE.clone(), attester_key.clone()),
                }],
                post_hook: Box::new(|_| {}),
            },
            SlotTestCase {
                transaction_test_cases: vec![TxTestCase {
                    outcome: TxOutcome::Applied(Box::new(move |ws: &mut WorkingSet<S>| {
                        // Check that the attestation succeeded
                        assert!(ws.events().iter().any(|event| matches!(
                            event.downcast_ref::<Event<S>>(),
                            Some(Event::ProcessedValidAttestation { .. })
                        )));
                        // Account for the gas used to send the attestation. We never attest to the current slot, so we don't add anything back.
                        expected_balance_ref_1
                            .fetch_sub(ws.gas_used_value(), std::sync::atomic::Ordering::SeqCst);
                    })),
                    message: MessageType::Plain(DUMMY_CALL_MESSAGE.clone(), attester_key.clone()),
                }],
                post_hook: Box::new(|_| {}),
            },
            SlotTestCase {
                transaction_test_cases: vec![TxTestCase {
                    outcome: TxOutcome::Applied(Box::new(move |ws: &mut WorkingSet<S>| {
                        // Check that the attestation succeeded
                        assert!(ws.events().iter().any(|event| matches!(
                            event.downcast_ref::<Event<S>>(),
                            Some(Event::ProcessedValidAttestation { .. })
                        )));
                        // Account for the gas used to send the attestation. We never attest to the current slot, so we don't add anything back.
                        expected_balance_ref_2
                            .fetch_sub(ws.gas_used_value(), std::sync::atomic::Ordering::SeqCst);
                    })),
                    message: MessageType::Plain(DUMMY_CALL_MESSAGE.clone(), attester_key.clone()),
                }],
                post_hook: Box::new(move |ws| {
                    // Check the attester's non-bonded balance
                    assert_eq!(
                        sov_bank::Bank::<S>::default()
                            .get_balance_of(&attester_address, sov_bank::GAS_TOKEN_ID, ws)
                            .unwrap(),
                        expected_balance_ref_3.load(std::sync::atomic::Ordering::SeqCst)
                    );
                    // Check that the attester still has their full bond
                    assert_eq!(
                        AttesterIncentives::<S, MockDaSpec>::default()
                            .get_bond_amount(attester_address, Role::Attester, ws)
                            .value,
                        initial_balance
                    );
                }),
            },
        ],
        AttesterRuntime::default(),
    );
}

#[test]
fn test_burn_on_invalid_attestation() {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
    let storage = storage_manager.create_storage();
    let state = WorkingSet::new(storage.clone());
    let (module, attester_address, _, sequencer, mut state) = setup(state);

    // Assert that the prover has the correct bond amount before processing the proof
    assert_eq!(
        module
            .get_bond_amount(attester_address, crate::call::Role::Attester, &mut state)
            .value,
        BOND_AMOUNT
    );

    // Simulate the execution of a chain, with the genesis hash and two transitions after.
    // Update the chain_state module and the optimistic module accordingly
    let state_checkpoint = state.checkpoint().0;
    commit_get_new_storage(storage, state_checkpoint, &mut storage_manager);
    let (mut exec_vars, state_checkpoint) = ExecutionSimulationVars::execute(
        3,
        &module,
        &mut storage_manager,
        &sequencer,
        &attester_address,
    );

    let transition_2 = exec_vars.pop().unwrap();
    let transition_1 = exec_vars.pop().unwrap();
    let initial_transition = exec_vars.pop().unwrap();

    let context = Context::<S>::new(attester_address, Default::default(), sequencer, 1);

    let mut state = state_checkpoint.to_working_set_unmetered();
    // Process an invalid proof for genesis: everything is correct except the storage proof.
    // Must simply return an error. Cannot burn the token at this point because we don't know if the
    // sender is bonded or not.
    {
        let attestation = Attestation {
            initial_state_root: initial_transition.state_root,
            slot_hash: [1; 32].into(),
            post_state_root: transition_1.state_root,
            proof_of_bond: sov_modules_api::optimistic::ProofOfBond {
                claimed_transition_num: INIT_HEIGHT + 1,
                proof: transition_1.state_proof.clone(),
            },
        };

        let attestation_error = module
            .process_attestation(&context, attestation.into(), &mut state)
            .unwrap_err();

        // The working set does not produce events because the method has returned an error
        assert_eq!(state.events().len(), 0);

        assert_eq!(
            attestation_error,
            AttesterIncentiveErrors::InvalidBondingProof,
            "The bonding proof should fail"
        );
    }

    // Assert that the prover's bond amount has not been burned
    assert_eq!(
        module
            .get_bond_amount(attester_address, crate::call::Role::Attester, &mut state)
            .value,
        BOND_AMOUNT
    );

    // Now process a valid attestation for genesis.
    {
        let attestation = Attestation {
            initial_state_root: initial_transition.state_root,
            slot_hash: [1; 32].into(),
            post_state_root: transition_1.state_root,
            proof_of_bond: sov_modules_api::optimistic::ProofOfBond {
                claimed_transition_num: INIT_HEIGHT + 1,
                proof: initial_transition.state_proof,
            },
        };

        module
            .process_attestation(&context, attestation.into(), &mut state)
            .expect("An invalid proof is an error");

        // The working set has only returned one event
        assert_eq!(state.events().len(), 1);

        // This is a valid attestation event.
        let valid_event = state.take_event(0).unwrap();
        let valid_event = valid_event.downcast::<crate::Event<S>>().unwrap();

        assert_eq!(
            valid_event,
            crate::Event::ProcessedValidAttestation {
                attester: attester_address
            }
        );
    }

    // Then process a new attestation having the wrong initial state root. The attester must be slashed, and the fees burnt
    {
        let attestation = Attestation {
            initial_state_root: initial_transition.state_root,
            slot_hash: [2; 32].into(),
            post_state_root: transition_2.state_root,
            proof_of_bond: sov_modules_api::optimistic::ProofOfBond {
                claimed_transition_num: INIT_HEIGHT + 2,
                proof: transition_1.state_proof.clone(),
            },
        };

        module
            .process_attestation(&context, attestation.into(), &mut state)
            .expect("Since we slash the user we must exit gracefully");

        // The working set has only returned one event
        assert_eq!(state.events().len(), 1);

        let slash_event = state.take_event(0).unwrap();
        let slash_event = slash_event.downcast::<crate::Event<S>>().unwrap();

        assert_eq!(
            slash_event,
            crate::Event::UserSlashed {
                address: attester_address,
                reason: crate::call::SlashingReason::InvalidInitialHash
            }
        );
    }

    // Check that the attester's bond has been burnt
    assert_eq!(
        module
            .get_bond_amount(attester_address, crate::call::Role::Attester, &mut state)
            .value,
        0
    );

    // Check that the attestation is not part of the challengeable set
    assert!(
        module
            .bad_transition_pool
            .get(&(INIT_HEIGHT + 2), &mut state)
            .is_none(),
        "The transition should not exist in the pool"
    );

    // Bond the attester once more
    module
        .bond_user_helper(
            BOND_AMOUNT,
            &attester_address,
            crate::call::Role::Attester,
            &mut state,
        )
        .unwrap();

    {
        // Check that the attester has been bonded again

        // The working set has only returned one event
        assert_eq!(state.events().len(), 1);

        let bond_event = state.take_event(0).unwrap();
        let bond_event = bond_event.downcast::<crate::Event<S>>().unwrap();

        assert_eq!(
            bond_event,
            crate::Event::BondedAttester {
                new_deposit: BOND_AMOUNT,
                total_bond: BOND_AMOUNT
            }
        );
    }

    // Process an attestation that has the right bonding proof and initial hash but has a faulty post transition hash.
    {
        let attestation = Attestation {
            initial_state_root: transition_1.state_root,
            slot_hash: [2; 32].into(),
            post_state_root: transition_1.state_root,
            proof_of_bond: sov_modules_api::optimistic::ProofOfBond {
                claimed_transition_num: INIT_HEIGHT + 2,
                proof: transition_1.state_proof,
            },
        };

        module
            .process_attestation(&context, attestation.into(), &mut state)
            .expect("Since we slash the user we must exit gracefully");

        // The working set has only returned one event
        assert_eq!(state.events().len(), 1);

        let slash_event = state.take_event(0).unwrap();
        let slash_event = slash_event.downcast::<crate::Event<S>>().unwrap();

        assert_eq!(
            slash_event,
            crate::Event::UserSlashed {
                address: attester_address,
                reason: crate::call::SlashingReason::TransitionInvalid
            }
        );
    }

    // Check that the attester's bond has been burnt
    assert_eq!(
        module
            .get_bond_amount(attester_address, crate::call::Role::Attester, &mut state)
            .value,
        0
    );

    // The attestation should be part of the challengeable set and its associated value should be the BOND_AMOUNT
    assert_eq!(
        module
            .bad_transition_pool
            .get(&(INIT_HEIGHT + 2), &mut state)
            .unwrap(),
        BOND_AMOUNT,
        "The transition should not exist in the pool"
    );
}
