use std::collections::HashMap;

pub use sov_attester_incentives;
pub use sov_attester_incentives::{
    AttesterIncentives, AttesterIncentivesConfig, CallMessage as AttesterCallMessage,
};
pub use sov_bank::{Bank, BankConfig, Coins, IntoPayable, Payable, TokenConfig, TokenId};
pub use sov_chain_state::ChainStateConfig;
use sov_db::schema::SchemaBatch;
use sov_db::storage_manager::NativeStorageManager;
pub use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockBlob, MockBlock, MockBlockHeader, MockDaSpec};
use sov_modules_api::{
    ApiStateAccessor, ApplySlotOutput, BlobData, CryptoSpec, DaSpec, EncodeCall, Genesis, Module,
    SlotData, Spec,
};
pub use sov_modules_stf_blueprint::GenesisParams;
use sov_modules_stf_blueprint::{BatchReceipt, Runtime, StfBlueprint};
pub use sov_prover_incentives::{ProverIncentives, ProverIncentivesConfig};
use sov_rollup_interface::da::RelevantBlobIters;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
pub use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
use sov_state::{DefaultStorageSpec, ProverStorage, Storage};
pub use sov_value_setter::{ValueSetter, ValueSetterConfig};

use crate::runtime::traits::EndSlotHookRegistry;
use crate::{
    MessageType, SlotExpectedResult, SlotTestCase, TestStfBlueprint, TxExpectedResult, TxTestCase,
};

pub(crate) mod macros;

/// Utilities for testing a runtime in the optimistic execution context.
pub mod optimistic;
/// Traits used to define interfaces for the runtime.
pub mod traits;
/// Defines a [`TestRuntimeWrapper`] which allows to override hooks using closures.
pub mod wrapper;
/// Utilities for testing a runtime in the ZK execution context.
pub mod zk;
use traits::{MinimalGenesis, PostTxHookRegistry};
pub use wrapper::{TestRuntimeWrapper, WorkingSetClosure};

type DefaultSpecWithHasher<S> = DefaultStorageSpec<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>;

/// Defines a slot receipt. A slot receipt is a list of [`BatchReceipt`]s.
pub type SlotReceipt = Vec<BatchReceipt>;

/// Stateful test runner that can be used to run and accumulate slot results for a given runtime.
pub struct TestRunner<RT: Runtime<S, MockDaSpec>, S: Spec> {
    stf: StfBlueprint<S, MockDaSpec, RT, BasicKernel<S, MockDaSpec>>,
    nonces: HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    slot_receipts: Vec<SlotReceipt>,
    state_root: <S::Storage as Storage>::Root,
    storage_manager: NativeStorageManager<MockDaSpec, ProverStorage<DefaultSpecWithHasher<S>>>,
    default_sequencer_da_address: <MockDaSpec as DaSpec>::Address,
}

/// Defines a slot runner. A slot runner is a list of [`BatchRunner`]s.
pub type SlotRunner<S, M> = Vec<BatchRunner<S, M>>;
/// Defines a batch runner. A batch runner is a list of [`TxRunner`]s.
pub type BatchRunner<S, M> = Vec<TxRunner<S, M>>;

/// Defines a transaction runner. A transaction runner is a [`MessageType`] and an [`TxExpectedResult`].
/// It is produced from a [`TxTestCase`].
pub struct TxRunner<S: Spec, M: Module> {
    pub(crate) message: MessageType<M, S>,
    pub(crate) expected_result: TxExpectedResult,
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

    // Register the transaction hooks with the runtime and builds a [`SlotRunner`] for each slot.
    fn register_hooks<M: Module>(
        &mut self,
        slots: Vec<SlotTestCase<RT, M, S>>,
    ) -> Vec<SlotRunner<S, M>> {
        let (slot_runners, post_slot_closures): (Vec<_>, Vec<_>) = slots
            .into_iter()
            .map(
                |SlotTestCase {
                     batch_test_cases,
                     post_hook,
                 }| {
                    let batch_runners: Vec<_> = batch_test_cases
                        .into_iter()
                        .map(|batch_test_case| {
                            let (batch_runners, post_checks): (Vec<_>, Vec<_>) =
                                batch_test_case.into_iter().map(TxTestCase::split).unzip();

                            self.runtime().add_post_dispatch_tx_hook_actions(
                                post_checks.into_iter().flatten().collect(),
                            );

                            batch_runners
                        })
                        .collect();

                    (batch_runners, post_hook)
                },
            )
            .unzip();

        self.runtime().add_end_slot_hook_actions(post_slot_closures);

        slot_runners
    }

    fn build_batch<M: Module>(
        &mut self,
        stf_state: &ProverStorage<
            DefaultStorageSpec<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>,
        >,
        slot_runner: Vec<Vec<TxRunner<S, M>>>,
    ) -> (Vec<MockBlob>, SlotExpectedResult)
    where
        RT: EncodeCall<M>,
    {
        let mut state = ApiStateAccessor::<S>::new(stf_state.clone());

        let (blobs, expected_slot_results): (Vec<_>, Vec<_>) = slot_runner
            .into_iter()
            .map(|batch_runner| {
                let build_batch_txs = |runner: TxRunner<S, M>| {
                    (
                        runner.message.to_raw_tx::<RT>(&mut self.nonces, &mut state),
                        runner.expected_result,
                    )
                };

                let (batch_of_raw_txs, expected_tx_results): (Vec<_>, Vec<_>) =
                    batch_runner.into_iter().map(build_batch_txs).unzip();

                let batch = BlobData::new_batch(batch_of_raw_txs);
                let blob = MockBlob::new_with_hash(
                    borsh::to_vec(&batch).unwrap(),
                    self.default_sequencer_da_address,
                );

                (blob, expected_tx_results)
            })
            .unzip();

        (blobs, expected_slot_results)
    }

    /// Checks the slot results and apply the changes to the state
    fn check_and_apply_slot_result(
        &mut self,
        block_header: MockBlockHeader,
        expected_slot_results: SlotExpectedResult,
        result: ApplySlotOutput<
            <S as Spec>::InnerZkvm,
            <S as Spec>::OuterZkvm,
            MockDaSpec,
            TestStfBlueprint<RT, S>,
        >,
    ) {
        let slot_receipts = result.batch_receipts;

        for (batch_receipt, expected_batch_results) in
            slot_receipts.iter().zip(expected_slot_results)
        {
            for (tx_receipt, expected_tx_result) in
                batch_receipt.tx_receipts.iter().zip(expected_batch_results)
            {
                match expected_tx_result {
                    TxExpectedResult::Applied => {
                        assert!(tx_receipt.receipt.is_successful());
                    }
                    TxExpectedResult::Reverted => {
                        assert!(tx_receipt.receipt.is_reverted());
                    }
                }
            }
        }

        self.storage_manager
            .save_change_set(&block_header, result.change_set, SchemaBatch::new())
            .unwrap();

        self.slot_receipts.push(slot_receipts);

        self.state_root = result.state_root;
    }

    /// Executes a single slot with a given setup function
    fn execute_slot<M: Module>(&mut self, slot_runner: SlotRunner<S, M>)
    where
        RT: EncodeCall<M>,
    {
        let block_header = MockBlockHeader::from_height(self.curr_slot_number() + 1);

        let (stf_state, _) = self
            .storage_manager
            .create_state_for(&block_header)
            .expect("Block builds on height zero");

        let (mut blobs, expected_slot_results) = self.build_batch(&stf_state, slot_runner);

        // TODO(@theochap): add support for proof blobs
        let relevant_blobs = RelevantBlobIters {
            proof_blobs: vec![],
            batch_blobs: blobs.iter_mut().collect(),
        };

        let result = self.stf.apply_slot(
            self.state_root(),
            stf_state,
            Default::default(),
            &block_header,
            &Default::default(),
            relevant_blobs,
        );

        self.check_and_apply_slot_result(block_header, expected_slot_results, result);
    }

    /// Executes the provided slots
    pub fn execute_slots<M: Module>(&mut self, slots_test_cases: Vec<SlotTestCase<RT, M, S>>)
    where
        RT: EncodeCall<M>,
    {
        let slots_runner = self.register_hooks(slots_test_cases);

        for slot_runner in slots_runner {
            self.execute_slot(slot_runner);
        }

        assert!(
            self.stf.runtime().try_get_next_tx_action().flatten().is_none(),
            "All post tx hooks must have run! This error indicates that at least one transaction failed that was expected to succeed!"
        );

        assert!(
            self.stf
                .runtime()
                .try_get_next_slot_action()
                .flatten()
                .is_none(),
            "All end slot hooks must have run! This should be unreachable!"
        );
    }

    /// Run a test on the given runtime
    ///
    /// The test is defined by a series of slot test cases, where the workflow is...
    /// 1. Run genesis
    /// 2. For each slot, apply the provided pre-execution closure to each call message
    /// with the current state as an argument. This allows us to set update any call messages
    /// that depend on the current state.
    /// 3. For each call message, execute the message and apply the post-execution closure to check
    /// that the result is valid.
    ///
    /// This method calls successively [`TestRunner::new_with_genesis`] followed by [`TestRunner::execute_slots`].
    pub fn run_test<M>(
        genesis_config: GenesisParams<
            <RT as Genesis>::Config,
            BasicKernelGenesisConfig<S, MockDaSpec>,
        >,
        slots: Vec<SlotTestCase<RT, M, S>>,
        runtime: RT,
    ) where
        RT: EncodeCall<M>,
        M: Module,
    {
        let mut runner = TestRunner::new_with_genesis(genesis_config, runtime);
        runner.execute_slots(slots);
    }
}
