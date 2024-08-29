use std::convert::Infallible;

use sov_attester_incentives::Attestation;
use sov_bank::{Bank, GAS_TOKEN_ID};
use sov_chain_state::ChainState;
use sov_mock_da::MockDaSpec;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::{
    ApiStateAccessor, DaSpec, ProofOutcome, ProofSerializer, SerializedAttestation,
    SerializedChallenge, SovApiProofSerializer, Spec, StateTransitionPublicData,
};
use sov_state::{Storage, StorageProof};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{AttesterIncentives, TestRunner};
use sov_test_utils::{
    assert_matches, generate_optimistic_runtime, AsUser, ProofTestCase, ProofType, TestAttester,
    TestChallenger, TestUser, TransactionType,
};

pub(crate) type S = sov_test_utils::TestSpec;

pub(crate) type TestAttesterIncentives = AttesterIncentives<S, MockDaSpec>;

pub(crate) type RT = TestRuntime<S, MockDaSpec>;

generate_optimistic_runtime!(TestRuntime <= );

pub type SetupParams = (
    TestRunner<RT, S>,
    TestAttester<S>,
    TestChallenger<S>,
    TestUser<S>,
);

/// Helper that sets up the tests and checks that the genesis state is valid.
pub(crate) fn setup() -> SetupParams {
    // Generate a genesis config, then overwrite the attester key/address with ones that
    // we know. We leave the other values untouched.
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let genesis_attester = genesis_config.initial_attester.clone();

    let attester_address = genesis_attester.user_info.address();
    let attester_bond = genesis_attester.bond;
    let attester_balance = genesis_attester.user_info.available_gas_balance;

    let genesis_challenger = genesis_config.initial_challenger.clone();

    let additional_account = genesis_config.additional_accounts.first().unwrap().clone();

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into());

    let mut runner =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), TestRuntime::default());

    runner.query_state(|state| {
        // Check that the attester account is bonded
        assert_eq!(
            TestAttesterIncentives::default()
                .bonded_attesters
                .get(&attester_address, state)
                .unwrap(),
            Some(attester_bond),
            "The genesis attester should be bonded"
        );

        // Check the balance of the attester is equal to the free balance
        assert_eq!(
            TestRunner::<RT, S>::bank_gas_balance(&attester_address, state),
            Some(attester_balance),
            "The balance of the attester should be equal to the free balance"
        );
    });

    (
        runner,
        genesis_attester,
        genesis_challenger,
        additional_account,
    )
}

pub(crate) fn consume_gas_tx_for_signer(signer: &TestUser<S>) -> TransactionType<Bank<S>, S> {
    let recipient = TestUser::<S>::generate(0);
    signer.create_plain_message(sov_bank::CallMessage::Transfer {
        to: recipient.address(),
        coins: sov_bank::Coins {
            amount: 1000,
            token_id: GAS_TOKEN_ID,
        },
    })
}

#[allow(clippy::type_complexity)]
pub(crate) fn build_proof(
    state: &mut ApiStateAccessor<S>,
    slot_to_attest: u64,
    user_address: &<S as Spec>::Address,
) -> Result<
    Attestation<
        <MockDaSpec as DaSpec>::SlotHash,
        <<S as Spec>::Storage as Storage>::Root,
        StorageProof<<<S as Spec>::Storage as Storage>::Proof>,
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

    let mut archival_state = state.get_archival_at(slot_to_attest);

    let proof_of_bond = TestAttesterIncentives::default()
        .bonded_attesters
        .get_with_proof(user_address, &mut archival_state);

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

#[allow(clippy::type_complexity)]
pub(crate) fn make_attestation_blob(
    attestation: Attestation<
        <MockDaSpec as DaSpec>::SlotHash,
        <<S as Spec>::Storage as Storage>::Root,
        StorageProof<<<S as Spec>::Storage as Storage>::Proof>,
    >,
) -> Vec<u8> {
    let serialized_attestation = SerializedAttestation::from_attestation(&attestation).unwrap();

    SovApiProofSerializer::<S>::new()
        .serialize_attestation_blob_with_metadata(serialized_attestation)
        .unwrap()
}

pub(crate) fn create_test_case(
    genesis_attester: TestAttester<S>,
    serialized_attestation: Vec<u8>,
) -> ProofTestCase<S, MockDaSpec> {
    ProofTestCase {
        input: ProofType::Inline(serialized_attestation),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_matches!(result.outcome.unwrap().outcome, ProofOutcome::Valid { .. });

            assert_eq!(
                TestAttesterIncentives::default()
                    .bonded_attesters
                    .get(&genesis_attester.user_info.address(), state)
                    .unwrap(),
                Some(genesis_attester.bond),
                "Bonded amount should not have changed"
            );

            // TODO #1292: check rewards.
        }),
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn build_challenge(
    state: &mut ApiStateAccessor<S>,
    challenge_slot: u64,
    prover_address: <S as Spec>::Address,
) -> Result<
    StateTransitionPublicData<
        <S as Spec>::Address,
        MockDaSpec,
        <<S as Spec>::Storage as Storage>::Root,
    >,
    Infallible,
> {
    let chain_state = ChainState::<S, MockDaSpec>::default();
    // Get the values for the transition being attested
    let current_transition = chain_state
        .get_historical_transitions(challenge_slot, state)?
        .unwrap();

    let prev_root = if challenge_slot == 1 {
        chain_state.get_genesis_hash(state)?
    } else {
        chain_state
            .get_historical_transitions(challenge_slot - 1, state)?
            .map(|t| *t.post_state_root())
    }
    .unwrap();

    let challenge: StateTransitionPublicData<
        _,
        MockDaSpec,
        <<S as Spec>::Storage as Storage>::Root,
    > = StateTransitionPublicData {
        initial_state_root: prev_root,
        final_state_root: *current_transition.post_state_root(),
        slot_hash: *current_transition.slot_hash(),
        validity_condition: *current_transition.validity_condition(),
        prover_address,
    };

    Ok(challenge)
}

#[allow(clippy::type_complexity)]
pub(crate) fn make_challenge_blob(
    challenge: StateTransitionPublicData<
        <S as Spec>::Address,
        MockDaSpec,
        <<S as Spec>::Storage as Storage>::Root,
    >,
    is_valid: bool,
    challenge_slot: u64,
) -> Vec<u8> {
    let serialized_challenge = MockZkvm::create_serialized_proof(is_valid, challenge);
    let serialized_challenge = SerializedChallenge {
        raw_challenge: serialized_challenge,
    };

    SovApiProofSerializer::<S>::new()
        .serialize_challenge_blob_with_metadata(serialized_challenge, challenge_slot)
        .unwrap()
}
