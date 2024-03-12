use std::sync::{Arc, RwLock};

use sov_attester_incentives::{AttesterIncentives, AttesterIncentivesConfig};
use sov_bank::{get_genesis_token_address, Bank, BankConfig, Coins, TokenConfig};
use sov_chain_state::ChainStateConfig;
use sov_mock_da::{MockBlob, MockBlock, MockBlockHeader, MockDaSpec, MockValidityCond};
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::da::Time;
use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::macros::DefaultRuntime;
use sov_modules_api::runtime::capabilities::{
    ContextResolver, GasEnforcer, Kernel, TransactionDeduplicator,
};
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{
    AccessoryStateCheckpoint, Context, DaSpec, DispatchCall, Event, Gas, GasArray, Genesis,
    MessageCodec, PublicKey, Spec, StateCheckpoint, WorkingSet, Zkvm,
};
use sov_modules_stf_blueprint::kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_modules_stf_blueprint::{
    BatchReceipt, GenesisParams, Runtime, SequencerOutcome, StfBlueprint,
};
use sov_prover_storage_manager::SimpleStorageManager;
use sov_rollup_interface::stf::{SlotResult, StateTransitionFunction};
use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
use sov_state::storage::{NativeStorage, StorageProof};
use sov_state::{DefaultStorageSpec, Storage};
use sov_value_setter::{ValueSetter, ValueSetterConfig};

type TestStf = StfBlueprint<
    S,
    MockDaSpec,
    MockZkVerifier,
    TestRuntime<S, MockDaSpec>,
    BasicKernel<S, MockDaSpec>,
>;
type BatchReceiptContents =
    <TestStf as StateTransitionFunction<<S as Spec>::Zkvm, Da>>::BatchReceiptContents;
type TxReceiptContents =
    <TestStf as StateTransitionFunction<<S as Spec>::Zkvm, Da>>::TxReceiptContents;

pub(crate) type S = sov_test_utils::TestSpec;
pub(crate) type Da = MockDaSpec;

#[derive(Genesis, DispatchCall, Event, MessageCodec, DefaultRuntime)]
#[serialization(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize
)]
pub(crate) struct TestRuntime<S: Spec, Da: DaSpec> {
    pub value_setter: ValueSetter<S>,
    pub sequencer_registry: SequencerRegistry<S, Da>,
    pub bank: Bank<S>,
    pub attester_incentives: AttesterIncentives<S, Da>,
}

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
            stake_amount: 10000,
            is_preferred_sequencer: true,
        }
    }
}

pub struct AttesterIncentivesParams<S: Spec, Da: DaSpec> {
    pub initial_attesters: Vec<(S::Address, u64)>,
    pub reward_token_supply_address: S::Address,
    pub rollup_finality_period: u64,
    pub minimum_attester_bond: u64,
    pub minimum_challenger_bond: u64,
    pub maximum_attested_height: u64,
    pub light_client_finalized_height: u64,
    pub commitment_to_allowed_challenge_method: <S::Zkvm as Zkvm>::CodeCommitment,
    pub validity_condition_checker: Da::Checker,
}

impl Default for AttesterIncentivesParams<S, MockDaSpec> {
    fn default() -> Self {
        AttesterIncentivesParams {
            initial_attesters: vec![([1; 32].into(), 0)],
            reward_token_supply_address: [0; 32].into(),
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
    pub salt: u64,
    pub init_balance: u64,
    pub addresses_and_balances: Vec<(<S as Spec>::Address, u64)>,
}

impl Default for BankParams {
    fn default() -> Self {
        BankParams {
            token_name: "TEST_TOKEN".to_string(),
            salt: 0,
            init_balance: 100000000,
            addresses_and_balances: Vec::new(),
        }
    }
}

impl<S: Spec, Da: DaSpec> Runtime<S, Da> for TestRuntime<S, Da> {
    type GenesisConfig = GenesisConfig<S, Da>;
    type GenesisPaths = ();
    fn rpc_methods(_storage: Arc<RwLock<S::Storage>>) -> jsonrpsee::RpcModule<()> {
        unimplemented!()
    }
    fn genesis_config(
        _genesis_paths: &Self::GenesisPaths,
    ) -> Result<Self::GenesisConfig, anyhow::Error> {
        unimplemented!()
    }
}

impl<S: Spec, Da: DaSpec> TxHooks for TestRuntime<S, Da> {
    type Spec = S;

    fn pre_dispatch_tx_hook(
        &self,
        _tx: &Transaction<Self::Spec>,
        _working_set: &mut sov_modules_api::WorkingSet<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn post_dispatch_tx_hook(
        &self,
        _tx: &Transaction<Self::Spec>,
        _ctx: &Context<S>,
        _working_set: &mut sov_modules_api::WorkingSet<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

impl<S: Spec, Da: DaSpec> ApplyBatchHooks<Da> for TestRuntime<S, Da> {
    type Spec = S;
    type BatchResult = SequencerOutcome<Da::Address>;

    fn begin_batch_hook(
        &self,
        _batch: &mut BatchWithId,
        _sender: &<Da as DaSpec>::Address,
        _state_checkpoint: &mut sov_modules_api::StateCheckpoint<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn end_batch_hook(
        &self,
        _result: Self::BatchResult,
        _working_set: &mut sov_modules_api::StateCheckpoint<S>,
    ) {
    }
}

impl<S: Spec, Da: DaSpec> SlotHooks for TestRuntime<S, Da> {
    type Spec = S;

    fn begin_slot_hook(
        &self,
        _pre_state_root: S::VisibleHash,
        _working_set: &mut sov_modules_api::VersionedStateReadWriter<StateCheckpoint<S>>,
    ) {
    }

    fn end_slot_hook(&self, _working_set: &mut sov_modules_api::StateCheckpoint<S>) {}
}

impl<S: Spec, Da: DaSpec> FinalizeHook for TestRuntime<S, Da> {
    type Spec = S;

    fn finalize_hook(
        &self,
        _root_hash: S::VisibleHash,
        _accesorry_working_set: &mut AccessoryStateCheckpoint<S>,
    ) {
    }
}

impl<S: Spec, Da: DaSpec> GasEnforcer<S, Da> for TestRuntime<S, Da> {
    /// The transaction type that the gas enforcer knows how to parse
    type Tx = Transaction<S>;
    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas(
        &self,
        tx: &Self::Tx,
        context: &Context<S>,
        gas_price: &<S::Gas as Gas>::Price,
        mut state_checkpoint: StateCheckpoint<S>,
    ) -> Result<WorkingSet<S>, StateCheckpoint<S>> {
        match self
            .bank
            .reserve_gas(tx, gas_price, context.sender(), &mut state_checkpoint)
        {
            Ok(gas_meter) => Ok(state_checkpoint.to_revertable(gas_meter)),
            Err(e) => {
                tracing::debug!("Unable to reserve gas from {}. {}", e, context.sender());
                Err(state_checkpoint)
            }
        }
    }

    /// Refunds any remaining gas to the payer after the transaction is processed.
    fn refund_remaining_gas(
        &self,
        tx: &Self::Tx,
        context: &Context<S>,
        gas_meter: &sov_modules_api::GasMeter<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.bank
            .refund_remaining_gas(tx, gas_meter, context.sender(), state_checkpoint);
    }
}

impl<S: Spec, Da: DaSpec> TransactionDeduplicator<S, Da> for TestRuntime<S, Da> {
    /// The transaction type that the deduplicator knows how to parse.
    type Tx = Transaction<S>;
    /// Prevents duplicate transactions from running.
    // TODO(@preston-evans98): Use type system to prevent writing to the `StateCheckpoint` during this check
    fn check_uniqueness(
        &self,
        _tx: &Self::Tx,
        _context: &Context<S>,
        _state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        _tx: &Self::Tx,
        _sequencer: &Da::Address,
        _state_checkpoint: &mut StateCheckpoint<S>,
    ) {
    }
}

/// Resolves the context for a transaction.
impl<S: Spec, Da: DaSpec> ContextResolver<S, Da> for TestRuntime<S, Da> {
    /// The transaction type that the resolver knows how to parse.
    type Tx = Transaction<S>;
    /// Resolves the context for a transaction.
    fn resolve_context(
        &self,
        tx: &Self::Tx,
        sequencer: &Da::Address,
        height: u64,
        working_set: &mut StateCheckpoint<S>,
    ) -> Context<S> {
        let sender = tx.pub_key().to_address();
        let sequencer = self
            .sequencer_registry
            .resolve_da_address(sequencer, working_set)
            .ok_or(anyhow::anyhow!("Sequencer was no longer registered by the time of context resolution. This is a bug")).unwrap();
        Context::<S>::new(sender, sequencer, height)
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
    stf: StfBlueprint<S, Da, MockZkVerifier, TestRuntime<S, Da>, TestKernel<S, Da>>,
    storage_manager: SimpleStorageManager<DefaultStorageSpec>,
}

impl TestRollup {
    pub(crate) fn stf(
        &self,
    ) -> &StfBlueprint<S, Da, MockZkVerifier, TestRuntime<S, Da>, TestKernel<S, Da>> {
        &self.stf
    }

    pub(crate) fn attester_incentives(&self) -> &AttesterIncentives<S, Da> {
        &self.stf().runtime().attester_incentives
    }

    pub(crate) fn bank(&self) -> &Bank<S> {
        &self.stf().runtime().bank
    }

    pub(crate) fn storage(&mut self) -> <S as Spec>::Storage {
        self.storage_manager.create_storage()
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
        let token_address =
            get_genesis_token_address::<S>(&bank_params.token_name, bank_params.salt);
        let runtime_config: <TestRuntime<S, Da> as Runtime<S, Da>>::GenesisConfig = GenesisConfig {
            value_setter: ValueSetterConfig {
                admin: admin_pub_key,
            },
            sequencer_registry: SequencerConfig {
                seq_rollup_address: seq_params.rollup_address,
                seq_da_address: seq_params.da_address,
                coins_to_lock: Coins {
                    amount: seq_params.stake_amount,
                    token_address,
                },
                is_preferred_sequencer: true,
            },
            bank: BankConfig {
                tokens: vec![TokenConfig {
                    token_name: bank_params.token_name.clone(),
                    token_address,
                    address_and_balances: {
                        let mut address_and_balances: Vec<(<S as Spec>::Address, u64)> =
                            bank_params
                                .addresses_and_balances
                                .clone()
                                .into_iter()
                                .collect();
                        let mut attester_balances = attester_params
                            .initial_attesters
                            .clone()
                            .into_iter()
                            .collect::<Vec<(<S as Spec>::Address, u64)>>();
                        attester_balances
                            .push((seq_params.rollup_address, bank_params.init_balance));
                        address_and_balances.append(&mut attester_balances);
                        address_and_balances
                    },

                    authorized_minters: vec![seq_params.rollup_address],
                }],
            },
            attester_incentives: AttesterIncentivesConfig {
                initial_attesters: attester_params.initial_attesters,
                bonding_token_address: token_address,
                reward_token_supply_address: attester_params.reward_token_supply_address,
                rollup_finality_period: attester_params.rollup_finality_period,
                minimum_attester_bond: attester_params.minimum_attester_bond,
                minimum_challenger_bond: attester_params.minimum_challenger_bond,
                maximum_attested_height: attester_params.maximum_attested_height,
                light_client_finalized_height: attester_params.light_client_finalized_height,
                commitment_to_allowed_challenge_method: attester_params
                    .commitment_to_allowed_challenge_method,
                validity_condition_checker: attester_params.validity_condition_checker,
                phantom_data: Default::default(),
            },
        };

        let kernel_config: <TestKernel<S, Da> as Kernel<S, Da>>::GenesisConfig =
            BasicKernelGenesisConfig {
                chain_state: ChainStateConfig {
                    current_time: Default::default(),
                    gas_price_blocks_depth: 10,
                    gas_price_maximum_elasticity: 1,
                    initial_gas_price: <<<S as Spec>::Gas as Gas>::Price as GasArray>::ZEROED,
                    minimum_gas_price: <<<S as Spec>::Gas as Gas>::Price as GasArray>::ZEROED,
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
                self.storage().get_with_proof(
                    self.stf()
                        .runtime()
                        .attester_incentives
                        .get_attester_storage_key(attester_address),
                )
            });

            // We apply a new transaction with the same values
            let mut slot: MockBlock = MockBlock {
                header: MockBlockHeader {
                    prev_hash: [(i + height) * 10; 32].into(),
                    hash: [(i + height + 1) * 10; 32].into(),
                    height: height.into(),
                    time: Time::now(),
                },
                validity_cond: MockValidityCond::default(),
                blobs: blobs.clone(),
            };

            let storage = self.storage();
            let SlotResult {
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
                &mut slot.blobs,
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
