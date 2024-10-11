use std::convert::Infallible;

use serde::Serialize;
use sov_bank::{config_gas_token_id, Bank};
use sov_chain_state::ChainState;
use sov_mock_da::MockValidityCond;
use sov_mock_zkvm::MockCodeCommitment;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::registration_lib::StakeRegistration;
use sov_modules_api::{
    AggregatedProofPublicData, ApiStateAccessor, CodeCommitment, ProofSerializer as _,
    SerializedAggregatedProof, SovApiProofSerializer, Spec,
};
use sov_prover_incentives::ProverIncentives;
use sov_test_utils::runtime::genesis::zk::config::HighLevelZkGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    generate_zk_runtime, AsUser, TestProver, TestSpec, TestUser, TransactionType,
};

pub(crate) type S = sov_test_utils::TestSpec;
pub(crate) type TestProverIncentives = ProverIncentives<S>;
pub(crate) type RT = TestRuntime<S>;
pub(crate) const MOCK_CODE_COMMITMENT: MockCodeCommitment = MockCodeCommitment([0u8; 32]);

generate_zk_runtime!(TestRuntime <= );

/// Returns the minimal bond required to register a prover at the current slot.
pub fn minimal_bond(runner: &TestRunner<TestRuntime<S>, S>) -> u64 {
    runner.query_state(|state| {
        TestProverIncentives::default()
            .get_minimum_bond(state)
            .unwrap_infallible()
            .unwrap()
    })
}

pub(crate) fn setup_with_custom_runtime(
    runtime: TestRuntime<S>,
) -> (TestRunner<RT, S>, TestProver<TestSpec>, TestUser<S>) {
    let minimal_genesis_config = HighLevelZkGenesisConfig::generate_with_additional_accounts(1);
    let unbonded_user = minimal_genesis_config
        .additional_accounts
        .first()
        .unwrap()
        .clone();
    let prover = minimal_genesis_config.initial_prover.clone();
    let genesis_config = GenesisConfig::from_minimal_config(minimal_genesis_config.into());
    let runner = TestRunner::new_with_genesis(genesis_config.into_genesis_params(), runtime);

    (runner, prover, unbonded_user)
}

/// Same as [`setup_with_custom_runtime`] but uses the default runtime.
pub(crate) fn setup() -> (TestRunner<RT, S>, TestProver<TestSpec>, TestUser<S>) {
    setup_with_custom_runtime(TestRuntime::default())
}

pub(crate) fn build_proof(
    state: &mut ApiStateAccessor<S>,
    initial_slot: u64,
    end_slot: u64,
    prover_address: <S as Spec>::Address,
) -> Result<AggregatedProofPublicData, Infallible> {
    let chain_state = ChainState::<S>::default();
    let genesis_hash = chain_state
        .get_genesis_hash(state)
        .unwrap()
        .expect("Genesis hash must be set");
    let initial_transition = chain_state
        .get_historical_transitions(initial_slot, state)
        .unwrap()
        .unwrap();
    let end_transition = chain_state
        .get_historical_transitions(end_slot, state)
        .unwrap()
        .unwrap();
    let vec_validity_cond = borsh::to_vec(&MockValidityCond { is_valid: true }).unwrap();

    Ok(AggregatedProofPublicData {
        validity_conditions: vec![
            vec_validity_cond.clone();
            (end_slot - initial_slot + 1) as usize
        ],
        initial_slot_number: initial_slot,
        final_slot_number: end_slot,
        initial_state_root: genesis_hash.as_ref().to_vec(),
        genesis_state_root: genesis_hash.as_ref().to_vec(),
        final_state_root: end_transition.post_state_root().as_ref().to_vec(),
        initial_slot_hash: initial_transition.slot_hash().as_ref().to_vec(),
        final_slot_hash: end_transition.slot_hash().as_ref().to_vec(),
        code_commitment: CodeCommitment(MOCK_CODE_COMMITMENT.0.to_vec()),
        rewarded_addresses: vec![prover_address.as_ref().to_vec()],
    })
}

pub(crate) fn consume_gas_tx_for_signer(signer: &TestUser<S>) -> TransactionType<Bank<S>, S> {
    let recipient = TestUser::<S>::generate(0);
    signer.create_plain_message(sov_bank::CallMessage::Transfer {
        to: recipient.address(),
        coins: sov_bank::Coins {
            amount: 1000,
            token_id: config_gas_token_id(),
        },
    })
}

pub(crate) fn serialize_proof<T: Serialize>(agg_proof: T) -> Vec<u8> {
    let proof = sov_mock_zkvm::MockZkvm::create_serialized_proof(true, agg_proof);
    let serialized_proof = SerializedAggregatedProof {
        raw_aggregated_proof: proof,
    };
    SovApiProofSerializer::<S>::new()
        .serialize_proof_blob_with_metadata(serialized_proof)
        .unwrap()
}
