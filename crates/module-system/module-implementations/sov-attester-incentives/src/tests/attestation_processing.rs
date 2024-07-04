use std::convert::Infallible;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_chain_state::ChainState;
use sov_mock_da::MockDaSpec;
use sov_modules_api::hooks::TxHooks;
use sov_modules_api::optimistic::Attestation;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{
    CryptoSpec, GasMeter, Module, PrivateKey, Spec, StateCheckpoint, UnmeteredStateWrapper,
    WorkingSet,
};
use sov_state::jmt::RootHash;
use sov_state::{BorshCodec, SlotValue, Storage, StorageProof, StorageRoot};
use sov_test_utils::runtime::genesis::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::sov_attester_incentives::{
    AttesterIncentives, CallMessage, Event, Role, WrappedAttestation,
};
use sov_test_utils::runtime::{
    run_test_with_setup_fn, MessageType, SlotTestCase, TxOutcome, TxTestCase,
};
use sov_test_utils::{generate_optimistic_runtime, TEST_DEFAULT_USER_STAKE};

type S = sov_test_utils::TestSpec;
const DUMMY_CALL_MESSAGE: CallMessage<S, MockDaSpec> = CallMessage::UnbondChallenger; // This will get overwritten by the setup hook

#[allow(clippy::type_complexity)]
fn create_attestation(
    slot_to_attest: u64,
    attester_address: &<S as Spec>::Address,
    state: &mut UnmeteredStateWrapper<WorkingSet<S>>,
) -> Result<
    Attestation<
        MockDaSpec,
        StorageProof<<<S as Spec>::Storage as Storage>::Proof>,
        <<S as Spec>::Storage as Storage>::Root,
    >,
    Infallible,
> {
    let chain_state = ChainState::<S, MockDaSpec>::default();

    // Get the values for the transition being attested
    let current_transition = chain_state
        .get_historical_transitions(slot_to_attest, state)?
        .unwrap();

    let prev_root = if slot_to_attest == 1 {
        chain_state.get_genesis_hash(state)?
    } else {
        chain_state
            .get_historical_transitions(slot_to_attest - 1, state)?
            .map(|t| *t.post_state_root())
    }
    .unwrap();

    let mut archival_ws = state.inner().get_archival_at(1);
    let mut archival_state = UnmeteredStateWrapper::new(&mut archival_ws);

    // Recall that the attester must be bonded *at a height that light clients consider finalized*
    // Since finalization takes a long time (24 hours), we know that it will never advance past slot 1 in tests.
    let proof_of_bond = AttesterIncentives::<S, MockDaSpec>::default()
        .bonded_attesters
        .get_with_proof(attester_address, &mut archival_state);

    Ok(Attestation {
        initial_state_root: prev_root,
        slot_hash: *current_transition.slot_hash(),
        post_state_root: *current_transition.post_state_root(),
        proof_of_bond: sov_modules_api::optimistic::ProofOfBond {
            claimed_transition_num: slot_to_attest,
            proof: proof_of_bond,
        },
    })
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
    let initial_balance = attester.bond;
    let expected_balance = attester.additional_balance.unwrap_or_default();

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into());
    let mut last_attested_slot = 0;

    // Create a transaction setup function which overwrites dummy CallMessages with a valid attestation.
    // We have to use the test frawework's setup hook to do this, since we don't know the correct state
    // roots in advance.
    let mut attestation_setup =
        move |message: &mut <AttesterIncentives<S, MockDaSpec> as Module>::CallMessage,
              _root: <<S as Spec>::Storage as Storage>::Root,
              mut state: UnmeteredStateWrapper<
            <AttesterRuntime<S, MockDaSpec> as TxHooks>::TxState,
        >| {
            if message == &DUMMY_CALL_MESSAGE {
                let next_slot = last_attested_slot + 1;
                let attestation = create_attestation(next_slot, &attester_address, &mut state)
                    .unwrap_infallible();
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
                post_hook: Box::new(move |state_checkpoint| {
                    assert_eq!(
                        AttesterIncentives::<S, MockDaSpec>::default()
                            .bonded_attesters
                            .get(&attester_address, state_checkpoint)
                            .unwrap_infallible()
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
                    outcome: TxOutcome::Applied(Box::new(
                        move |ws: UnmeteredStateWrapper<WorkingSet<S>>| {
                            // Do accounting for the attester's balance
                            {
                                // The attester's balance should be decremented by the gas used
                                expected_balance.fetch_sub(
                                    ws.inner().gas_used_value(),
                                    std::sync::atomic::Ordering::SeqCst,
                                );
                                // We know that attester will attest to this slot later, so he'll get back some of his gas at that point.
                                expected_balance.fetch_add(
                                    AttesterIncentives::<S, MockDaSpec>::default()
                                        .burn_rate()
                                        .apply(ws.inner().gas_used_value()),
                                    std::sync::atomic::Ordering::SeqCst,
                                );
                            }

                            // Check that the attestation succeeded
                            assert!(ws.inner().events().iter().any(|event| matches!(
                                event.downcast_ref::<Event<S>>(),
                                Some(Event::ProcessedValidAttestation { .. })
                            )));
                        },
                    )),
                    message: MessageType::Plain(DUMMY_CALL_MESSAGE.clone(), attester_key.clone()),
                }],
                post_hook: Box::new(|_| {}),
            },
            SlotTestCase {
                transaction_test_cases: vec![TxTestCase {
                    outcome: TxOutcome::Applied(Box::new(
                        move |ws: UnmeteredStateWrapper<WorkingSet<S>>| {
                            // Check that the attestation succeeded
                            assert!(ws.inner().events().iter().any(|event| matches!(
                                event.downcast_ref::<Event<S>>(),
                                Some(Event::ProcessedValidAttestation { .. })
                            )));
                            // Account for the gas used to send the attestation. We never attest to the current slot, so we don't add anything back.
                            expected_balance_ref_1.fetch_sub(
                                ws.inner().gas_used_value(),
                                std::sync::atomic::Ordering::SeqCst,
                            );
                        },
                    )),
                    message: MessageType::Plain(DUMMY_CALL_MESSAGE.clone(), attester_key.clone()),
                }],
                post_hook: Box::new(|_| {}),
            },
            SlotTestCase {
                transaction_test_cases: vec![TxTestCase {
                    outcome: TxOutcome::Applied(Box::new(
                        move |ws: UnmeteredStateWrapper<WorkingSet<S>>| {
                            // Check that the attestation succeeded
                            assert!(ws.inner().events().iter().any(|event| matches!(
                                event.downcast_ref::<Event<S>>(),
                                Some(Event::ProcessedValidAttestation { .. })
                            )));
                            // Account for the gas used to send the attestation. We never attest to the current slot, so we don't add anything back.
                            expected_balance_ref_2.fetch_sub(
                                ws.inner().gas_used_value(),
                                std::sync::atomic::Ordering::SeqCst,
                            );
                        },
                    )),
                    message: MessageType::Plain(DUMMY_CALL_MESSAGE.clone(), attester_key.clone()),
                }],
                post_hook: Box::new(move |state_checkpoint| {
                    assert_eq!(
                        sov_bank::Bank::<S>::default()
                            .get_balance_of(
                                &attester_address,
                                sov_bank::GAS_TOKEN_ID,
                                state_checkpoint
                            )
                            .unwrap_infallible()
                            .unwrap(),
                        expected_balance_ref_3.load(std::sync::atomic::Ordering::SeqCst)
                    );

                    // Check that the attester still has their full bond
                    assert_eq!(
                        AttesterIncentives::<S, MockDaSpec>::default()
                            .get_bond_amount(attester_address, Role::Attester, state_checkpoint)
                            .unwrap_infallible()
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
    generate_optimistic_runtime!(AttesterRuntime <=);
    type RT = AttesterRuntime<S, MockDaSpec>;
    // Generate a genesis config, then overwrite the attester key/address with ones that
    // we know. We leave the other values untouched.
    let mut genesis_config = HighLevelOptimisticGenesisConfig::generate();
    let attester = &mut genesis_config.initial_attester;
    let attester_key = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate();
    attester.address = <S as Spec>::Address::from(&attester_key.pub_key());
    attester.bond = TEST_DEFAULT_USER_STAKE;
    let attester_address = attester.address;

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into());
    let mut round = 0;
    let mut last_attested_slot = 0;

    // Create a transaction setup function which overwrites dummy CallMessages with a valid attestation.
    // We have to use the test frawework's setup hook to do this, since we don't know the correct state
    // roots in advance.
    let mut attestation_setup =
        move |message: &mut <AttesterIncentives<S, MockDaSpec> as Module>::CallMessage,
              _root: <<S as Spec>::Storage as Storage>::Root,
              mut state: UnmeteredStateWrapper<
            <AttesterRuntime<S, MockDaSpec> as TxHooks>::TxState,
        >| {
            if message == &DUMMY_CALL_MESSAGE {
                let next_slot = last_attested_slot + 1;
                let mut attestation = create_attestation(next_slot, &attester_address, &mut state)
                    .unwrap_infallible();

                match round {
                    0 => {
                        // Process an invalid proof for genesis: everything is correct except the storage proof.
                        // Must simply return an error. Cannot burn the token at this point because we don't know if the
                        // sender is bonded or not.
                        attestation.proof_of_bond.proof.value =
                            Some(SlotValue::new(&(TEST_DEFAULT_USER_STAKE * 5), &BorshCodec));
                    }
                    1 => last_attested_slot += 1, // Since this attestation is unmodified, it will succeed so we need to move attesting to the next slot
                    2 => {
                        // Here we'll process an attestation with the wrong initial state root
                        attestation.initial_state_root =
                            StorageRoot::new(RootHash([255; 32]), RootHash([255; 32]));
                    }
                    3 => {
                        // Here we'll process an attestation with the wrong post state root
                        attestation.post_state_root =
                            StorageRoot::new(RootHash([255; 32]), RootHash([255; 32]));
                    }
                    _ => unreachable!(),
                };

                *message =
                    CallMessage::ProcessAttestation(WrappedAttestation { inner: attestation });
                round += 1;
            }
        };

    run_test_with_setup_fn(
        genesis.into_genesis_params(),
        &mut attestation_setup,
        vec![
            // Run any empty slot, and check that the attester has the correct bond amount from genesis
            SlotTestCase::<_, AttesterIncentives<S, MockDaSpec>, _> {
                transaction_test_cases: vec![],
                post_hook: Box::new(move |ws: &mut StateCheckpoint<S>| {
                    // Assert that genesis yielded the expected bond amount
                    assert_eq!(
                        AttesterIncentives::<S, MockDaSpec>::default()
                            .bonded_attesters
                            .get(&attester_address, ws)
                            .unwrap_infallible()
                            .unwrap_or_default(),
                        TEST_DEFAULT_USER_STAKE,
                    );
                }),
            },
            // Run an empty slot
            SlotTestCase::empty(),
            SlotTestCase {
                transaction_test_cases: vec![TxTestCase {
                    outcome: TxOutcome::Reverted, // Fails without slashing because the bond proof was invalid
                    message: MessageType::Plain(DUMMY_CALL_MESSAGE.clone(), attester_key.clone()),
                }],
                post_hook: Box::new(move |state| {
                    // Assert that the attester was not slashed
                    assert_eq!(
                        AttesterIncentives::<S, MockDaSpec>::default()
                            .get_bond_amount(attester_address, Role::Attester, state)
                            .unwrap_infallible()
                            .value,
                        TEST_DEFAULT_USER_STAKE,
                    );
                }),
            },
            SlotTestCase::from_txs(vec![
                TxTestCase {
                    outcome: TxOutcome::<RT>::Applied(Box::new(|state| {
                        // Check that the attestation succeeded
                        assert!(state.inner().events().iter().any(|event| matches!(
                            event.downcast_ref::<Event<S>>(),
                            Some(Event::ProcessedValidAttestation { .. })
                        )));
                    })),
                    message: MessageType::Plain(DUMMY_CALL_MESSAGE.clone(), attester_key.clone()),
                },
                TxTestCase {
                    outcome: TxOutcome::<RT>::Applied(Box::new(move |mut state| {
                        // Check that the attestation resulted in slashing
                        assert!(state.inner().events().iter().any(|event| matches!(
                            event.downcast_ref::<Event<S>>(),
                            Some(Event::UserSlashed { .. })
                        )));
                        // Assert that the attester was slashed
                        assert_eq!(
                            AttesterIncentives::<S, MockDaSpec>::default()
                                .get_bond_amount(attester_address, Role::Attester, &mut state)
                                .unwrap_infallible()
                                .value,
                            0,
                        );
                        // Check that the invalid attestation is not part of the challengeable set.
                        // (Since it has the wrong pre-state, no one will be fooled by it so we don't reward challengers)
                        assert!(
                            AttesterIncentives::<S, MockDaSpec>::default()
                                .bad_transition_pool
                                .get(&2, &mut state)
                                .unwrap_infallible()
                                .is_none(),
                            "The transition should not exist in the pool"
                        );
                    })),
                    message: MessageType::Plain(DUMMY_CALL_MESSAGE.clone(), attester_key.clone()),
                },
            ]),
            SlotTestCase::from_txs(vec![
                TxTestCase {
                    outcome: TxOutcome::<RT>::Applied(Box::new(move |mut state| {
                        assert!(state.inner().events().iter().any(|event| matches!(
                            event.downcast_ref::<Event<S>>(),
                            Some(Event::BondedAttester { .. })
                        )));
                        assert_eq!(
                            AttesterIncentives::<S, MockDaSpec>::default()
                                .get_bond_amount(attester_address, Role::Attester, &mut state)
                                .unwrap_infallible()
                                .value,
                            TEST_DEFAULT_USER_STAKE,
                        );
                    })),
                    message: MessageType::Plain(
                        CallMessage::BondAttester(TEST_DEFAULT_USER_STAKE),
                        attester_key.clone(),
                    ),
                },
                TxTestCase {
                    outcome: TxOutcome::<RT>::Applied(Box::new(move |mut state| {
                        // Check that the attestation resulted in slashing
                        assert!(state.inner().events().iter().any(|event| matches!(
                            event.downcast_ref::<Event<S>>(),
                            Some(Event::UserSlashed { .. })
                        )));
                        // Assert that the attester was slashed
                        assert_eq!(
                            AttesterIncentives::<S, MockDaSpec>::default()
                                .get_bond_amount(attester_address, Role::Attester, &mut state)
                                .unwrap_infallible()
                                .value,
                            0,
                        );
                        // The attestation should be part of the challengeable set and its associated value should be the BOND_AMOUNT
                        assert_eq!(
                            AttesterIncentives::<S, MockDaSpec>::default()
                                .bad_transition_pool
                                .get(&2, &mut state)
                                .unwrap_infallible(),
                            Some(TEST_DEFAULT_USER_STAKE),
                            "The transition should not exist in the pool"
                        );
                    })),
                    message: MessageType::Plain(DUMMY_CALL_MESSAGE.clone(), attester_key.clone()),
                },
            ]),
        ],
        AttesterRuntime::default(),
    );
}
