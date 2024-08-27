use std::collections::HashMap;

pub use sov_accounts::Accounts;
pub use sov_attester_incentives;
pub use sov_attester_incentives::{
    AttesterIncentives, AttesterIncentivesConfig, CallMessage as AttesterCallMessage,
};
use sov_bank::GAS_TOKEN_ID;
pub use sov_bank::{Bank, BankConfig, Coins, IntoPayable, Payable, TokenConfig, TokenId};
pub use sov_chain_state::ChainStateConfig;
use sov_db::schema::SchemaBatch;
use sov_db::storage_manager::{NativeChangeSet, NativeStorageManager};
pub use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockBlob, MockBlock, MockBlockHeader, MockDaSpec};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{
    ApiStateAccessor, ApplySlotOutput, Batch, CryptoSpec, DaSpec, EncodeCall, Gas, GasArray,
    Genesis, InfallibleStateAccessor, Module, RuntimeEventProcessor, SlotData, Spec,
};
pub use sov_modules_stf_blueprint::GenesisParams;
use sov_modules_stf_blueprint::{Runtime, StfBlueprint, TransactionReceipt};
pub use sov_prover_incentives::{ProverIncentives, ProverIncentivesConfig};
use sov_rollup_interface::da::RelevantBlobs;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
pub use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
use sov_state::{DefaultStorageSpec, ProverStorage, Storage};
pub use sov_value_setter::{ValueSetter, ValueSetterConfig};

use crate::runtime::traits::EndSlotHookRegistry;
use crate::{
    generate_optimistic_runtime, BatchAssertContext, BatchReceipt, BatchTestCase,
    ProofAssertContext, ProofTestCase, ProofType, SlotInput, TestStfBlueprint,
    TransactionAssertContext, TransactionTestCase, TransactionType,
};

pub(crate) mod macros;

generate_optimistic_runtime!(TestOptimisticRuntime <= value_setter: ValueSetter<S>);

/// Utilities for generating genesis configs.
pub mod genesis;

/// Traits used to define interfaces for the runtime.
pub mod traits;
/// Defines a [`TestRuntimeWrapper`] which allows to override hooks using closures.
pub mod wrapper;
use traits::{MinimalGenesis, PostTxHookRegistry};
pub use wrapper::{TestRuntimeWrapper, WorkingSetClosure};

#[cfg(test)]
mod tests;

type DefaultSpecWithHasher<S> = DefaultStorageSpec<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>;

/// Defines a slot receipt. A slot receipt is a list of [`BatchReceipt`]s and a block header.
pub struct SlotReceipt<Da: DaSpec> {
    block_header: Da::BlockHeader,
    batch_receipts: Vec<BatchReceipt<Da>>,
}

impl<Da: DaSpec> SlotReceipt<Da> {
    /// Returns the last batch receipt in the slot receipt.
    pub fn last_batch_receipt(&self) -> &BatchReceipt<Da> {
        self.batch_receipts.last().unwrap()
    }

    /// Returns the last transaction receipt in the last batch receipt of the slot receipt.
    pub fn last_tx_receipt(&self) -> &TransactionReceipt {
        self.last_batch_receipt().tx_receipts.last().unwrap()
    }
}

/// Stateful test runner that can be used to run and accumulate slot results for a given runtime.
pub struct TestRunner<RT: Runtime<S, MockDaSpec>, S: Spec> {
    stf: StfBlueprint<S, MockDaSpec, RT, BasicKernel<S, MockDaSpec>>,
    nonces: HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    slot_receipts: Vec<SlotReceipt<MockDaSpec>>,
    state_root: <S::Storage as Storage>::Root,
    storage_manager: NativeStorageManager<MockDaSpec, ProverStorage<DefaultSpecWithHasher<S>>>,
    default_sequencer_da_address: <MockDaSpec as DaSpec>::Address,
}

type TestApplySlotOutput<RT, S> = ApplySlotOutput<
    <S as Spec>::InnerZkvm,
    <S as Spec>::OuterZkvm,
    MockDaSpec,
    TestStfBlueprint<RT, S>,
>;

/// The output of the runner
pub struct RunnerOutput<S: Spec> {
    /// The slot receipt emitted at the end of the slot execution
    pub receipt: SlotReceipt<MockDaSpec>,
    /// The change set containing the delta of the state after the slot execution
    pub change_set: NativeChangeSet,
    /// The root of the state after the slot execution
    pub root: <<S as Spec>::Storage as Storage>::Root,
}

impl<RT, S> TestRunner<RT, S>
where
    RT: Runtime<S, MockDaSpec>
        + PostTxHookRegistry<S, MockDaSpec>
        + EndSlotHookRegistry<S, MockDaSpec>
        + MinimalGenesis<S, Da = MockDaSpec>,
    S: Spec<Storage = ProverStorage<DefaultSpecWithHasher<S>>>,
{
    /// Returns the runtime of the test runner.
    pub fn runtime(&self) -> &RT {
        self.stf.runtime()
    }

    /// Returns the state root of the previous slot.
    /// Since genesis is always ran when constructing the runner, there will always be a previous slot when executing new slots.
    pub fn state_root(&self) -> &<S::Storage as Storage>::Root {
        &self.state_root
    }

    /// Returns the current slot number. The genesis slot is 0 but since genesis doesn't generate a receipt, we need to return the length of the execution slot receipts + 1.
    pub fn curr_slot_number(&self) -> u64 {
        self.slot_receipts.len() as u64 + 1
    }

    /// A simple helper function to get the balance of a given address in the gas token currency with an [`InfallibleStateAccessor`].
    /// This can be used to check the balance of an address in closures.
    pub fn bank_gas_balance(
        address: &S::Address,
        state: &mut impl InfallibleStateAccessor,
    ) -> Option<u64> {
        sov_bank::Bank::<S>::default()
            .get_balance_of(address, GAS_TOKEN_ID, state)
            .unwrap_infallible()
    }

    /// Returns the slot receipts accumulated by the state runner
    pub fn receipts(&self) -> &Vec<SlotReceipt<MockDaSpec>> {
        &self.slot_receipts
    }

    /// Returns a reference to the nonces used by the state runner
    pub fn nonces(&self) -> &HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64> {
        &self.nonces
    }

    fn current_state(&mut self) -> ApiStateAccessor<S> {
        let (stf_state, _) = if let Some(slot_receipt) = self.slot_receipts.last() {
            self.storage_manager
                .create_state_after(&slot_receipt.block_header)
        } else {
            self.storage_manager.create_bootstrap_state()
        }
        .expect("Impossible to create queryiable state. This is a bug.");

        ApiStateAccessor::<S>::new(stf_state)
    }

    /// Queries the state of the rollup. Calls the given closure with an [`ApiStateAccessor`] and returns the result.
    /// This method does not commit any changes to the state, it simply queries the state and discards the changes
    /// like what would happen by sending RPC/REST requests.
    ///
    /// ## Note
    /// We are using a closure here to ensure that we are accessing the most recent state of the rollup.
    /// Simply returning the [`ApiStateAccessor`] would not be sufficient because the state may be updated while
    /// the [`ApiStateAccessor`] still exists.
    pub fn query_state<Output>(
        &mut self,
        query: impl FnOnce(&mut ApiStateAccessor<S>) -> Output,
    ) -> Output {
        query(&mut self.current_state())
    }

    /// Builds a new test runner and runs genesis.
    pub fn new_with_genesis(
        genesis_config: GenesisParams<
            <RT as Genesis>::Config,
            BasicKernelGenesisConfig<S, MockDaSpec>,
        >,
        runtime: RT,
    ) -> Self {
        // Use the runtime to create an STF blueprint
        let stf =
            StfBlueprint::<S, MockDaSpec, RT, BasicKernel<S, MockDaSpec>>::with_runtime(runtime);

        // ----- Setup and run genesis ---------
        let temp_dir = tempfile::tempdir().unwrap();
        let mut storage_manager = NativeStorageManager::new(temp_dir.path())
            .expect("ProverStorageManager initialization has failed");

        let default_sequencer_da_address =
            <RT as MinimalGenesis<S>>::sequencer_registry_config(&genesis_config.runtime)
                .seq_da_address;

        let genesis_block = MockBlock::default();
        let (stf_state, _) = storage_manager
            .create_state_for(genesis_block.header())
            .unwrap();
        let (state_root, change_set) = stf.init_chain(stf_state, genesis_config);

        storage_manager
            .save_change_set(genesis_block.header(), change_set, SchemaBatch::new())
            .unwrap();
        // Write it to the database immediately
        storage_manager.finalize(&genesis_block.header).unwrap();

        // ----- End genesis ---------

        Self {
            nonces: HashMap::new(),
            slot_receipts: Vec::new(),
            state_root,
            storage_manager,
            stf,
            default_sequencer_da_address,
        }
    }

    fn next_header(&self) -> MockBlockHeader {
        MockBlockHeader::from_height(self.curr_slot_number() + 1)
    }

    fn txs_to_blobs<M: Module>(
        &mut self,
        txs: Vec<TransactionType<M, S>>,
        stf_state: &ProverStorage<
            DefaultStorageSpec<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>,
        >,
        sequencer: <MockDaSpec as DaSpec>::Address,
    ) -> RelevantBlobs<MockBlob>
    where
        RT: EncodeCall<M>,
    {
        let mut state = ApiStateAccessor::<S>::new(stf_state.clone());
        let raw_txns = txs
            .into_iter()
            .map(|tx| tx.to_raw_tx::<RT>(&mut self.nonces, &mut state))
            .collect::<Vec<_>>();
        let batch = Batch::new(raw_txns);
        let blob = MockBlob::new_with_hash(borsh::to_vec(&batch).unwrap(), sequencer);
        RelevantBlobs {
            batch_blobs: vec![blob],
            proof_blobs: vec![],
        }
    }

    /// Simulates execution of the provided input without committing to the updated state.
    /// This is useful to retreive non-deterministic outcomes associated with execution such as
    /// dynamic gas prices.
    pub fn simulate<T: Into<SlotInput<S, M>>, M>(
        &mut self,
        input: T,
        override_sequencer: Option<<MockDaSpec as DaSpec>::Address>,
    ) -> (TestApplySlotOutput<RT, S>, MockBlockHeader)
    where
        M: Module,
        RT: EncodeCall<M>,
    {
        let block_header = self.next_header();
        let (stf_state, _) = self
            .storage_manager
            .create_state_for(&block_header)
            .expect("Block builds on height zero");
        let slot_input: SlotInput<S, M> = input.into();
        let sequencer = override_sequencer.unwrap_or(self.default_sequencer_da_address);
        let mut blobs = match slot_input {
            SlotInput::Transaction(tx) => self.txs_to_blobs(vec![tx], &stf_state, sequencer),
            SlotInput::Batch(batch) => self.txs_to_blobs(batch.0, &stf_state, sequencer),
            SlotInput::Proof(proof) => {
                let proof_bytes = match proof {
                    ProofType::Inline(proof) => proof,
                    ProofType::Configuration(proof) => {
                        proof.from_state(&mut ApiStateAccessor::<S>::new(stf_state.clone()))
                    }
                };
                let blob = MockBlob::new_with_hash(proof_bytes, sequencer);
                RelevantBlobs {
                    batch_blobs: vec![],
                    proof_blobs: vec![blob],
                }
            }
        };
        (
            self.stf.apply_slot(
                self.state_root(),
                stf_state.clone(),
                Default::default(),
                &block_header,
                &Default::default(),
                blobs.as_iters(),
            ),
            block_header,
        )
    }

    /// Executes the provided input and commits the state updates.
    /// This is useful for executing setup transactions that aren't test cases.
    pub fn execute<T: Into<SlotInput<S, M>>, M>(
        &mut self,
        input: T,
        override_sequencer: Option<<MockDaSpec as DaSpec>::Address>,
    ) -> TestApplySlotOutput<RT, S>
    where
        M: Module,
        RT: EncodeCall<M>,
    {
        let (result, block_header) = self.simulate(input, override_sequencer);
        self.commit_apply_slot_output(&result, &block_header);
        result
    }

    fn commit_apply_slot_output(
        &mut self,
        output: &TestApplySlotOutput<RT, S>,
        header: &MockBlockHeader,
    ) {
        self.storage_manager
            .save_change_set(header, output.change_set.clone(), SchemaBatch::new())
            .unwrap();
        self.state_root = output.state_root;
        self.slot_receipts.push(SlotReceipt {
            block_header: header.clone(),
            batch_receipts: output.batch_receipts.clone(),
        });
    }

    /// Advance the rollup `slots_to_advance` slots without executing
    /// any transactions.
    pub fn advance_slots(&mut self, slots_to_advance: usize) -> &mut Self {
        for _ in 0..slots_to_advance {
            let block_header = self.next_header();
            let (stf_state, _) = self
                .storage_manager
                .create_state_for(&block_header)
                .expect("Block builds on height zero");
            let mut blobs = RelevantBlobs {
                proof_blobs: vec![],
                batch_blobs: vec![],
            };
            let result = self.stf.apply_slot(
                self.state_root(),
                stf_state.clone(),
                Default::default(),
                &block_header,
                &Default::default(),
                blobs.as_iters(),
            );
            self.commit_apply_slot_output(&result, &block_header);
        }
        self
    }

    /// Execute a [`TransactionTestCase`] against the current state of the test runtime.
    ///
    /// Under the hood this will execute a slot with a single batch containing a single
    /// transaction.
    pub fn execute_transaction<M: Module>(
        &mut self,
        transaction_test: TransactionTestCase<S, RT, M>,
    ) -> &mut Self
    where
        RT: EncodeCall<M> + RuntimeEventProcessor,
    {
        let result = self.execute(transaction_test.input, None);
        let batch_receipt = result.batch_receipts[0].clone();
        let tx_receipt = batch_receipt.tx_receipts[0].clone();
        let gas_used = <S as Spec>::Gas::from_slice(&tx_receipt.gas_used);
        let gas_price =
            <<S as Spec>::Gas as sov_modules_api::Gas>::Price::from_slice(&batch_receipt.gas_price);
        let ctx = TransactionAssertContext::from_receipt::<S, MockDaSpec>(
            tx_receipt,
            gas_used.value(&gas_price),
        );
        (transaction_test.assert)(ctx, &mut self.current_state());
        self
    }

    /// Execute a BatchTestCase against the current state of the runtime.
    ///
    /// Under the hood this will execute a slot with the provided batch.
    pub fn execute_batch<M: Module>(
        &mut self,
        batch_test: BatchTestCase<S, MockDaSpec, M>,
    ) -> &mut Self
    where
        RT: EncodeCall<M>,
    {
        let sender_da_address = batch_test
            .override_sequencer
            .unwrap_or(self.default_sequencer_da_address);
        let result = self.execute(batch_test.input, Some(sender_da_address));
        let ctx = BatchAssertContext {
            sender_da_address,
            outcome: result.batch_receipts.first().cloned(),
        };
        (batch_test.assert)(ctx, &mut self.current_state());
        self
    }

    /// Execute a ProofTestCase against the current state of the runtime.
    ///
    /// This will submit a slot containing a single proof blob.
    pub fn execute_proof<M: Module>(
        &mut self,
        proof_test: ProofTestCase<S, MockDaSpec>,
    ) -> &mut Self
    where
        RT: EncodeCall<M>,
    {
        let sender_da_address = proof_test
            .override_sequencer
            .unwrap_or(self.default_sequencer_da_address);
        let result = self.execute(proof_test.input, Some(sender_da_address));
        let ctx = ProofAssertContext {
            outcome: result.proof_receipts.first().cloned(),
        };
        (proof_test.assert)(ctx, &mut self.current_state());
        self
    }
}
