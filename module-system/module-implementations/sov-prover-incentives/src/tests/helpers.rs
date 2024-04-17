use sov_bank::IntoPayable;
use sov_chain_state::StateTransition;
use sov_mock_da::{MockBlockHeader, MockDaSpec, MockHash, MockValidityCond};
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier};
use sov_modules_api::da::Time;
use sov_modules_api::digest::Digest;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{
    Address, CryptoSpec, GasArray, GasPrice, KernelModule, KernelWorkingSet, Module, ModuleInfo,
    PrivateKey, Spec, StateCheckpoint, WorkingSet,
};
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::jmt::RootHash;
use sov_state::{DefaultStorageSpec, StorageRoot};

use crate::ProverIncentives;

pub(crate) type S = sov_modules_api::default_spec::DefaultSpec<MockZkVerifier, MockZkVerifier>;
pub(crate) type Da = MockDaSpec;

pub(crate) const BOND_AMOUNT: u64 = 1000;
pub(crate) const INITIAL_PROVER_BALANCE: u64 = 5 * BOND_AMOUNT;
pub(crate) const INITIAL_SEQUENCER_BALANCE: u64 = 20 * BOND_AMOUNT;
pub(crate) const MOCK_CODE_COMMITMENT: MockCodeCommitment = MockCodeCommitment([0u8; 32]);

pub const MAX_TX_GAS_AMOUNT: u64 = 100;
pub const TX_GAS_CONSUMED: [u64; 2] = [10; 2];
pub const TX_GAS_PRICE: [u64; 2] = [1; 2];

/// Generates an address by hashing the provided `key`.
pub fn generate_address(key: &str) -> <S as Spec>::Address {
    let hash: [u8; 32] =
        <<S as Spec>::CryptoSpec as CryptoSpec>::Hasher::digest(key.as_bytes()).into();
    Address::from(hash)
}

fn create_bank_config() -> (
    sov_bank::BankConfig<S>,
    <S as Spec>::Address,
    <S as Spec>::Address,
) {
    let prover_address = generate_address("prover_pub_key");
    let sequencer_address = generate_address("sequencer_pub_key");

    let token_config = sov_bank::GasTokenConfig {
        token_name: "InitialToken".to_owned(),
        address_and_balances: vec![
            (prover_address, INITIAL_PROVER_BALANCE),
            (sequencer_address, INITIAL_SEQUENCER_BALANCE),
        ],
        authorized_minters: vec![],
    };

    (
        sov_bank::BankConfig {
            gas_token_config: token_config,
            tokens: vec![],
        },
        prover_address,
        sequencer_address,
    )
}

/// Simulates the execution of the chain state by applying `steps` state transitions.
pub(crate) fn simulate_chain_state_execution(
    module: &ProverIncentives<S, Da>,
    sequencer: <S as Spec>::Address,
    steps: u8,
    gas_used_per_step: &<S as Spec>::Gas,
    state_checkpoint: &mut StateCheckpoint<S>,
) {
    let mut kernel_working_set = KernelWorkingSet::uninitialized(state_checkpoint);
    for i in 0..steps {
        let slot_header = MockBlockHeader {
            prev_hash: MockHash([i * 10; 32]),
            hash: MockHash([(i + 1) * 10; 32]),
            height: u64::from(i),
            time: Time::now(),
        };
        // We also need to call the `GasEnforcer` hook to ensure that the reward pool is populated.
        let tx_key = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate();

        let tx = Transaction::<S>::new(
            tx_key.pub_key(),
            vec![],
            tx_key.sign(&[]),
            0,
            0.into(),
            MAX_TX_GAS_AMOUNT,
            Some(<S as Spec>::Gas::from_slice(&TX_GAS_CONSUMED)),
            i.into(),
        );

        // We first need to reserve gas for the transaction
        let mut gas_meter = module
            .bank
            .reserve_gas(
                &tx,
                &GasPrice::from_slice(&TX_GAS_PRICE),
                &sequencer,
                kernel_working_set.inner,
            )
            .expect("Gas reserve failed");

        module.chain_state.begin_slot_hook(
            &slot_header,
            &MockValidityCond { is_valid: true },
            &StorageRoot::<DefaultStorageSpec>::new(RootHash([i; 32]), RootHash([i; 32])),
            &mut kernel_working_set,
        );

        // We charge some gas to the sequencer to make sure the gas meter is updated
        gas_meter
            .charge_gas(&<S as Spec>::Gas::from_slice(&TX_GAS_CONSUMED))
            .expect("Gas charge failed");

        module
            .chain_state
            .end_slot_hook(gas_used_per_step, &mut kernel_working_set);
        module.bank.refund_remaining_gas(
            &tx,
            &gas_meter,
            &sequencer,
            &module.id().to_payable(),
            &module.id().to_payable(),
            kernel_working_set.inner,
        );
    }
}

fn setup_helper(
    mut working_set: WorkingSet<S>,
) -> (ProverIncentives<S, Da>, Address, Address, WorkingSet<S>) {
    // Initialize bank
    let (bank_config, prover_address, sequencer) = create_bank_config();
    let bank = sov_bank::Bank::<S>::default();
    bank.genesis(&bank_config, &mut working_set)
        .expect("bank genesis must succeed");

    // Initialize chain state
    let chain_state_config = sov_chain_state::ChainStateConfig {
        current_time: Time::now(),
        initial_base_fee_per_gas: GasPrice::<2>::from(TX_GAS_PRICE),
    };

    let chain_state = sov_chain_state::ChainState::<S, Da>::default();

    let (mut checkpoint, meter, _) = working_set.checkpoint();

    let mut kernel_working_set = KernelWorkingSet::uninitialized(&mut checkpoint);
    chain_state
        .genesis_unchecked(&chain_state_config, &mut kernel_working_set)
        .expect("chain state genesis must succeed");

    // initialize prover incentives
    let module = ProverIncentives::<S, Da>::default();
    let config = crate::ProverIncentivesConfig {
        proving_penalty: BOND_AMOUNT / 2,
        minimum_bond: BOND_AMOUNT,
        commitment_of_allowed_verifier_method: MockCodeCommitment([0u8; 32]),
        initial_provers: vec![(prover_address, BOND_AMOUNT)],
    };

    let mut working_set = checkpoint.to_revertable(meter);

    module
        .genesis(&config, &mut working_set)
        .expect("prover incentives genesis must succeed");
    (module, prover_address, sequencer, working_set)
}

pub(crate) fn setup() -> (
    crate::ProverIncentives<S, sov_mock_da::MockDaSpec>,
    <S as Spec>::Address,
    <S as Spec>::Address,
    WorkingSet<S>,
) {
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    let (module, prover_address, sequencer, mut working_set) = setup_helper(working_set);

    // Assert that the prover has the correct bond amount before processing the proof
    assert_eq!(
        module
            .get_bond_amount(prover_address, &mut working_set)
            .unwrap()
            .value,
        BOND_AMOUNT
    );

    // We clear the events before processing the proof
    working_set.take_events();

    (module, prover_address, sequencer, working_set)
}

pub(crate) fn get_transition_unwrap(
    transition_num: u64,
    module: &ProverIncentives<S, Da>,
    working_set: &mut WorkingSet<S>,
) -> StateTransition<S, Da> {
    module
        .chain_state
        .get_historical_transitions(transition_num, working_set)
        .expect("transition must exist")
}
