use sov_attester_incentives::{AttesterIncentives, AttesterIncentivesConfig};
use sov_bank::{Bank, BankConfig, GasTokenConfig};
use sov_chain_state::{ChainState, ChainStateConfig};
use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockBlob, MockBlock, MockBlockHeader, MockDaSpec, MockValidityCond};
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier};
use sov_modules_api::da::Time;
use sov_modules_api::macros::config_value;
use sov_modules_api::namespaces::User;
use sov_modules_api::runtime::capabilities::Kernel;
use sov_modules_api::{DaSpec, Gas, Spec, StateCheckpoint, Zkvm};
use sov_modules_stf_blueprint::{BatchReceipt, GenesisParams, Runtime, StfBlueprint};
use sov_prover_storage_manager::SimpleStorageManager;
use sov_rollup_interface::stf::{ApplySlotOutput, StateTransitionFunction};
use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
use sov_state::storage::{NativeStorage, StorageProof};
use sov_state::{DefaultStorageSpec, Storage};
use sov_test_utils::runtime::{GenesisConfig, TestRuntime};
use sov_value_setter::ValueSetterConfig;

type TestStf = StfBlueprint<S, MockDaSpec, TestRuntime<S, MockDaSpec>, BasicKernel<S, MockDaSpec>>;
type BatchReceiptContents =
    <TestStf as StateTransitionFunction<MockZkVerifier, MockZkVerifier, Da>>::BatchReceiptContents;
type TxReceiptContents =
    <TestStf as StateTransitionFunction<MockZkVerifier, MockZkVerifier, Da>>::TxReceiptContents;

pub(crate) type S = sov_test_utils::TestSpec;
pub(crate) type Da = MockDaSpec;

pub(crate) const DEFAULT_STAKE_AMOUNT: u64 = 100;

pub(crate) const GAS_TX_FIXED_COST: [u64; 2] = config_value!("GAS_TX_FIXED_COST");

pub struct SequencerParams<S: Spec, Da: DaSpec> {
    pub rollup_address: S::Address,
    pub da_address: Da::Address,
    pub stake_amount: u64,
    pub is_preferred_sequencer: bool,
}

impl Default for SequencerParams<S, MockDaSpec> {
    fn default() -> Self {
        SequencerParams {
            rollup_address: [1_u8; 32].into(),
            da_address: [1_u8; 32].into(),
            stake_amount: 100,
            is_preferred_sequencer: true,
        }
    }
}

pub struct AttesterIncentivesParams<S: Spec, Da: DaSpec> {
    pub initial_attesters: Vec<(S::Address, u64)>,
    pub rollup_finality_period: u64,
    pub minimum_attester_bond: u64,
    pub minimum_challenger_bond: u64,
    pub maximum_attested_height: u64,
    pub light_client_finalized_height: u64,
    pub commitment_to_allowed_challenge_method: <S::InnerZkvm as Zkvm>::CodeCommitment,
    pub validity_condition_checker: Da::Checker,
}

impl Default for AttesterIncentivesParams<S, MockDaSpec> {
    fn default() -> Self {
        AttesterIncentivesParams {
            initial_attesters: vec![([1; 32].into(), 0)],
            rollup_finality_period: 0,
            minimum_attester_bond: 0,
            minimum_challenger_bond: 0,
            maximum_attested_height: 0,
            light_client_finalized_height: 0,
            commitment_to_allowed_challenge_method: MockCodeCommitment([0; 32]),
            validity_condition_checker: <MockDaSpec as DaSpec>::Checker::default(),
        }
    }
}

pub struct BankParams {
    pub token_name: String,
    pub init_balance: u64,
    pub addresses_and_balances: Vec<(<S as Spec>::Address, u64)>,
}

impl BankParams {
    /// Creates a new `BankParams` with a default `token_name` and `init_balance`.
    /// The `addresses_and_balances` are used to initialize the token balances.
    pub(crate) fn with_addresses_and_balances(
        addresses_and_balances: Vec<(<S as Spec>::Address, u64)>,
    ) -> Self {
        Self {
            token_name: String::from("TEST_TOKEN"),
            init_balance: 100000000,
            addresses_and_balances,
        }
    }
}

impl Default for BankParams {
    fn default() -> Self {
        BankParams {
            token_name: "TEST_TOKEN".to_string(),
            init_balance: 100000000,
            addresses_and_balances: Vec::new(),
        }
    }
}

pub(crate) type TestKernel<S, Da> = BasicKernel<S, Da>;

#[derive(Clone, Debug)]
pub(crate) struct ExecutionSimulationVars {
    pub state_root: <<S as Spec>::Storage as Storage>::Root,
    pub batch_receipts: Vec<BatchReceipt<BatchReceiptContents, TxReceiptContents>>,
    pub state_proof: Option<StorageProof<<<S as Spec>::Storage as Storage>::Proof>>,
}

pub(crate) struct TestRollup {
    stf: StfBlueprint<S, Da, TestRuntime<S, Da>, TestKernel<S, Da>>,
    storage_manager: SimpleStorageManager<DefaultStorageSpec>,
}

impl TestRollup {
    pub(crate) fn stf(&self) -> &StfBlueprint<S, Da, TestRuntime<S, Da>, TestKernel<S, Da>> {
        &self.stf
    }

    pub(crate) fn kernel(&self) -> &TestKernel<S, Da> {
        self.stf().kernel()
    }

    pub(crate) fn initial_base_fee_per_gas(&self) -> <<S as Spec>::Gas as Gas>::Price {
        ChainState::<S, Da>::initial_base_fee_per_gas()
    }

    pub(crate) fn attester_incentives(&self) -> &AttesterIncentives<S, Da> {
        &self.stf().runtime().attester_incentives
    }

    pub(crate) fn bank(&self) -> &Bank<S> {
        &self.stf().runtime().bank
    }

    pub(crate) fn sequencer_registry(&self) -> &SequencerRegistry<S, Da> {
        &self.stf().runtime().sequencer_registry
    }

    pub(crate) fn storage(&mut self) -> <S as Spec>::Storage {
        self.storage_manager.create_storage()
    }

    pub(crate) fn new_state_checkpoint(&mut self) -> StateCheckpoint<S> {
        StateCheckpoint::new(self.storage().clone())
    }

    pub(crate) fn storage_manager(&mut self) -> &mut SimpleStorageManager<DefaultStorageSpec> {
        &mut self.storage_manager
    }

    fn create_genesis_config(
        admin_pub_key: <S as Spec>::Address,
        seq_params: SequencerParams<S, Da>,
        bank_params: BankParams,
        attester_params: AttesterIncentivesParams<S, Da>,
    ) -> GenesisParams<GenesisConfig<S, Da>, BasicKernelGenesisConfig<S, Da>> {
        let runtime_config: <TestRuntime<S, Da> as Runtime<S, Da>>::GenesisConfig = GenesisConfig {
            value_setter: ValueSetterConfig {
                admin: admin_pub_key,
            },
            sequencer_registry: SequencerConfig {
                seq_rollup_address: seq_params.rollup_address,
                seq_da_address: seq_params.da_address,
                minimum_bond: seq_params.stake_amount,
                is_preferred_sequencer: true,
            },
            bank: BankConfig {
                gas_token_config: GasTokenConfig {
                    token_name: bank_params.token_name.clone(),
                    address_and_balances: bank_params.addresses_and_balances,
                    authorized_minters: vec![seq_params.rollup_address],
                },
                tokens: vec![],
            },
            attester_incentives: AttesterIncentivesConfig {
                initial_attesters: attester_params.initial_attesters,
                rollup_finality_period: attester_params.rollup_finality_period,
                minimum_attester_bond: attester_params.minimum_attester_bond,
                minimum_challenger_bond: attester_params.minimum_challenger_bond,
                maximum_attested_height: attester_params.maximum_attested_height,
                light_client_finalized_height: attester_params.light_client_finalized_height,
                validity_condition_checker: attester_params.validity_condition_checker,
                phantom_data: Default::default(),
            },
        };

        let kernel_config: <TestKernel<S, Da> as Kernel<S, Da>>::GenesisConfig =
            BasicKernelGenesisConfig {
                chain_state: ChainStateConfig {
                    current_time: Default::default(),
                    // The rollup code commitment is the same as the attester incentives challenge commitment
                    inner_code_commitment: attester_params.commitment_to_allowed_challenge_method,
                    outer_code_commitment: MockCodeCommitment::default(),
                    genesis_da_height: 0,
                },
            };
        GenesisParams {
            runtime: runtime_config,
            kernel: kernel_config,
        }
    }

    pub(crate) fn genesis(
        &mut self,
        admin_pub_key: <S as Spec>::Address,
        seq_params: SequencerParams<S, Da>,
        bank_params: BankParams,
        attester_params: AttesterIncentivesParams<S, Da>,
    ) -> <<S as Spec>::Storage as Storage>::Root {
        let storage = self.storage();
        let (init_root_hash, stf_change_set) = self.stf.init_chain(
            storage,
            Self::create_genesis_config(admin_pub_key, seq_params, bank_params, attester_params),
        );
        self.storage_manager.commit(stf_change_set);

        init_root_hash
    }

    pub(crate) fn new_from_path(path: &std::path::Path) -> Self {
        TestRollup {
            stf: TestStf::new(),
            storage_manager: SimpleStorageManager::new(path),
        }
    }

    pub(crate) fn new() -> Self {
        let tmpdir = tempfile::tempdir().unwrap();
        Self::new_from_path(tmpdir.path())
    }

    /// Generate an execution simulation for a given number of rounds. Returns a list of the successive state roots
    /// with associated bonding proofs for the associated attester address (if supplied).
    /// The state proof provide a bounding proof for the attester *before* the execution of each batch.
    pub(crate) fn execution_simulation(
        &mut self,
        rounds: u8,
        mut prev_root_hash: <<S as Spec>::Storage as Storage>::Root,
        blobs: Vec<MockBlob>,
        height: u8,
        attester_address: Option<<S as Spec>::Address>,
    ) -> Vec<ExecutionSimulationVars> {
        let mut ret_exec_vars = Vec::<ExecutionSimulationVars>::new();

        for i in 0..rounds {
            let state_proof = attester_address.map(|attester_address| {
                self.storage().get_with_proof::<User>(
                    self.stf()
                        .runtime()
                        .attester_incentives
                        .get_attester_storage_key(attester_address),
                    None,
                )
            });

            // We apply a new transaction with the same values
            let slot: MockBlock = MockBlock {
                header: MockBlockHeader {
                    prev_hash: [(i + height) * 10; 32].into(),
                    hash: [(i + height + 1) * 10; 32].into(),
                    height: height.into(),
                    time: Time::now(),
                },
                validity_cond: MockValidityCond::default(),
                batch_blobs: blobs.clone(),
                proof_blobs: Default::default(),
            };

            let storage = self.storage();

            let ApplySlotOutput {
                state_root: new_root_hash,
                change_set,
                batch_receipts,
                ..
            } = self.stf.apply_slot(
                &prev_root_hash,
                storage,
                Default::default(),
                &slot.header,
                &slot.validity_cond,
                slot.as_relevant_blobs().as_iters(),
            );

            self.storage_manager.commit(change_set);

            prev_root_hash = new_root_hash;

            ret_exec_vars.push(ExecutionSimulationVars {
                state_root: new_root_hash,
                batch_receipts,
                state_proof,
            });
        }

        ret_exec_vars
    }
}
