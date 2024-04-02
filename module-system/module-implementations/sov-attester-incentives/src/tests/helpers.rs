use sov_bank::{BankConfig, GasTokenConfig};
use sov_mock_da::{
    MockBlock, MockBlockHeader, MockDaSpec, MockValidityCond, MockValidityCondChecker,
};
use sov_mock_zkvm::MockCodeCommitment;
use sov_modules_api::namespaces::User;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::utils::generate_address;
use sov_modules_api::{
    Address, CryptoSpec, Gas, GasArray, Genesis, KernelModule, KernelWorkingSet, PrivateKey, Spec,
    WorkingSet,
};
use sov_modules_core::runtime::capabilities::mocks::MockKernel;
use sov_modules_core::{GasMeter, StateCheckpoint};
use sov_rollup_interface::da::Time;
use sov_state::storage::{NativeStorage, Storage, StorageProof};
use sov_state::{DefaultStorageSpec, ProverStorage, SparseMerkleProof, StorageRoot};

use crate::AttesterIncentives;

type S = sov_test_utils::TestSpec;

pub const TOKEN_NAME: &str = "TEST_TOKEN";
pub const BOND_AMOUNT: u64 = 1000;
pub const INITIAL_BOND_AMOUNT: u64 = 10 * BOND_AMOUNT;
pub const DEFAULT_ROLLUP_FINALITY: u64 = 3;
pub const INIT_HEIGHT: u64 = 0;

pub const MAX_TX_GAS_AMOUNT: u64 = 100;
pub const TX_GAS_CONSUMED: [u64; 2] = [10; 2];
pub const TX_GAS_PRICE: [u64; 2] = [1; 2];

pub const NUM_BANK_ACCOUNTS: usize = 3;

/// Consumes and commit the existing working set on the underlying storage
/// `storage` must be the underlying storage defined on the working set for this method to work.
pub(crate) fn commit_get_new_state_checkpoint(
    storage: &ProverStorage<DefaultStorageSpec>,
    checkpoint: StateCheckpoint<S>,
) -> (StorageRoot<DefaultStorageSpec>, StateCheckpoint<S>) {
    let (reads_writes, _, witness) = checkpoint.freeze();

    let new_root = storage
        .validate_and_commit(reads_writes, &witness)
        .expect("Should be able to commit");

    (new_root, StateCheckpoint::new(storage.clone()))
}

pub(crate) fn create_bank_config_with_token(
    token_name: String,
    addresses_count: usize,
    initial_balance: u64,
) -> (BankConfig<S>, Vec<Address>) {
    let address_and_balances: Vec<(Address, u64)> = (0..addresses_count)
        .map(|i| {
            let key = format!("key_{}", i);
            let addr = generate_address::<S>(&key);
            (addr, initial_balance)
        })
        .collect();

    let token_config = GasTokenConfig {
        token_name,
        address_and_balances: address_and_balances.clone(),
        authorized_minters: vec![address_and_balances.first().unwrap().0],
    };

    (
        BankConfig {
            gas_token_config: token_config,
            tokens: vec![],
        },
        address_and_balances
            .into_iter()
            .map(|(addr, _)| addr)
            .collect(),
    )
}

/// Creates a bank config with a token, and a prover incentives module.
/// Returns the prover incentives module and the attester and challenger's addresses.
#[allow(clippy::type_complexity)]
pub(crate) fn setup(
    mut working_set: WorkingSet<S>,
) -> (
    AttesterIncentives<S, MockDaSpec>,
    Address,
    Address,
    Address,
    WorkingSet<S>,
) {
    // Initialize bank
    let (bank_config, mut addresses) = create_bank_config_with_token(
        TOKEN_NAME.to_string(),
        NUM_BANK_ACCOUNTS,
        INITIAL_BOND_AMOUNT,
    );
    let bank = sov_bank::Bank::<S>::default();
    bank.genesis(&bank_config, &mut working_set)
        .expect("bank genesis must succeed");

    let attester_address = addresses.pop().unwrap();
    let challenger_address = addresses.pop().unwrap();
    let sequencer = addresses.pop().unwrap();

    // Initialize chain state
    let chain_state_config = sov_chain_state::ChainStateConfig {
        current_time: Default::default(),
        gas_price_blocks_depth: 10,
        gas_price_maximum_elasticity: 1,
        initial_gas_price: [1, 1].into(),
        minimum_gas_price: [1, 1].into(),
    };

    let mut state_checkpoint = working_set.checkpoint().0;
    let chain_state = sov_chain_state::ChainState::<S, MockDaSpec>::default();
    chain_state
        .genesis_unchecked(
            &chain_state_config,
            &mut KernelWorkingSet::uninitialized(&mut state_checkpoint),
        )
        .expect("Chain state genesis must succeed");

    let mut working_set = state_checkpoint.to_revertable(GasMeter::unmetered());
    // initialize prover incentives
    let module = AttesterIncentives::<S, MockDaSpec>::default();
    let config = crate::AttesterIncentivesConfig {
        minimum_attester_bond: BOND_AMOUNT,
        minimum_challenger_bond: BOND_AMOUNT,
        commitment_to_allowed_challenge_method: MockCodeCommitment([0u8; 32]),
        initial_attesters: vec![(attester_address, BOND_AMOUNT)],
        rollup_finality_period: DEFAULT_ROLLUP_FINALITY,
        maximum_attested_height: INIT_HEIGHT,
        light_client_finalized_height: INIT_HEIGHT,
        validity_condition_checker: MockValidityCondChecker::<MockValidityCond>::new(),
        phantom_data: Default::default(),
    };

    module
        .genesis(&config, &mut working_set)
        .expect("prover incentives genesis must succeed");

    (
        module,
        attester_address,
        challenger_address,
        sequencer,
        working_set,
    )
}

pub(crate) struct ExecutionSimulationVars {
    pub state_root: StorageRoot<DefaultStorageSpec>,
    pub state_proof:
        StorageProof<SparseMerkleProof<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>>,
}

impl ExecutionSimulationVars {
    /// Simple function that returns the gas reward for a transaction execution.
    pub(crate) fn tx_reward() -> u64 {
        <S as Spec>::Gas::from_slice(&TX_GAS_CONSUMED)
            .value(&<<S as Spec>::Gas as Gas>::Price::from_slice(&TX_GAS_PRICE))
    }

    /// Generate an execution simulation for a given number of rounds. Returns a list of the successive state roots
    /// with associated bonding proofs.
    /// The execution simulation also performs a gas charge and refund for the sequencer, which locks reward to the attester module.
    pub(crate) fn execute(
        rounds: u8,
        module: &AttesterIncentives<S, MockDaSpec>,
        storage: &ProverStorage<DefaultStorageSpec>,
        sequencer: &<S as Spec>::Address,
        attester_address: &<S as Spec>::Address,
        mut state_checkpoint: StateCheckpoint<S>,
    ) -> (
        // Vector of the successive state roots with associated bonding proofs
        Vec<Self>,
        StateCheckpoint<S>,
    ) {
        let mut ret_exec_vars = Vec::<ExecutionSimulationVars>::new();

        for i in 0..rounds {
            // Commit the working set
            let (root_hash, w_set) = commit_get_new_state_checkpoint(storage, state_checkpoint);
            state_checkpoint = w_set;

            let bond_proof = storage
                .get_with_proof::<User>(module.get_attester_storage_key(*attester_address), None);

            ret_exec_vars.push(ExecutionSimulationVars {
                state_root: root_hash,
                state_proof: bond_proof,
            });

            // Then process the first transaction. Only sets the genesis hash and a transition in progress.
            let slot_data = MockBlock {
                header: MockBlockHeader {
                    prev_hash: [i; 32].into(),
                    hash: [i + 1; 32].into(),
                    height: INIT_HEIGHT + u64::from(i + 1),
                    time: Time::now(),
                },
                validity_cond: MockValidityCond { is_valid: true },
                batch_blobs: Default::default(),
                proof_blobs: Default::default(),
            };
            let kernel = MockKernel::<S, MockDaSpec>::new(i as u64, i as u64);

            // We also need to call the `GasEnforcer` hook to ensure that the reward pool is populated.
            let tx_key = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate();

            let tx = Transaction::<S>::new(
                tx_key.pub_key(),
                vec![],
                tx_key.sign(&[]),
                0,
                0,
                MAX_TX_GAS_AMOUNT,
                Some(<<S as Spec>::Gas as Gas>::Price::from_slice(&TX_GAS_PRICE)),
                i.into(),
            );

            // We first need to reserve gas for the transaction
            let mut gas_meter = module
                .reserve_gas(
                    &tx,
                    tx.max_gas_price().unwrap(),
                    sequencer,
                    &mut state_checkpoint,
                )
                .expect("Gas reserve failed");

            let mut kernel_working_set =
                KernelWorkingSet::from_kernel(&kernel, &mut state_checkpoint);

            // Then we execute the chain state to make sure the transition data is persisted
            module.chain_state.begin_slot_hook(
                &slot_data.header,
                &slot_data.validity_cond,
                &root_hash,
                &mut kernel_working_set,
            );

            // We charge some gas to the sequencer to make sure the gas meter is updated
            gas_meter
                .charge_gas(&<S as Spec>::Gas::from_slice(&TX_GAS_CONSUMED))
                .expect("Gas charge failed");

            module
                .chain_state
                .end_slot_hook(gas_meter.gas_used(), &mut kernel_working_set);

            // Then we can refund some gas to the sequencer
            module.refund_remaining_gas(&tx, &gas_meter, sequencer, &mut state_checkpoint);
        }

        (ret_exec_vars, state_checkpoint)
    }
}
