use std::collections::HashMap;
use std::sync::Arc;

pub use sov_accounts::Accounts;
pub use sov_attester_incentives;
pub use sov_attester_incentives::{
    AttesterIncentives, AttesterIncentivesConfig, CallMessage as AttesterCallMessage,
};
pub use sov_bank::{
    config_gas_token_id, Bank, BankConfig, CallMessage as BankCallMessage, Coins, IntoPayable,
    Payable, TokenConfig, TokenId,
};
use sov_blob_storage::PreferredBatchData;
pub use sov_capabilities::StandardProvenRollupCapabilities;
pub use sov_chain_state::ChainStateConfig;
use sov_db::storage_manager::NativeChangeSet;
pub use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockAddress, MockBlob, MockBlockHeader, MockDaSpec};
use sov_modules_api::capabilities::{KernelSlotHooks, KernelWithSlotMapping};
use sov_modules_api::da::Time;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{
    ApiStateAccessor, ApplySlotOutput, Batch, BlobDataWithId, CryptoSpec, DaSpec, EncodeCall,
    Error, Gas, Genesis, InfallibleStateAccessor, KernelStateAccessor, Module,
    RuntimeEventProcessor, Spec, StateCheckpoint, TxEffect,
};
use sov_modules_stf_blueprint::{
    get_gas_used, StfBlueprint, TransactionReceipt, TxReceiptContents,
};
pub use sov_modules_stf_blueprint::{GenesisParams, Runtime, RuntimeEndpoints};
pub use sov_nonces::Nonces;
pub use sov_prover_incentives::{ProverIncentives, ProverIncentivesConfig};
use sov_rollup_interface::da::RelevantBlobs;
use sov_rollup_interface::stf::{ExecutionContext, StateTransitionFunction};
pub use sov_sequencer_registry::{SequencerConfig, SequencerRegistry, SequencerStakeMeter};
use sov_state::{DefaultStorageSpec, ProverStorage, Storage};
pub use sov_value_setter::{ValueSetter, ValueSetterConfig};
pub use tokio::sync::watch::Receiver;

use crate::storage::SimpleStorageManager;
use crate::{
    generate_optimistic_runtime, BatchAssertContext, BatchReceipt, BatchTestCase, BatchType,
    ProofAssertContext, ProofTestCase, SequencerInfo, SlotInput, SoftConfirmationBlobInfo,
    TestStfBlueprintWithKernel, TransactionAssertContext, TransactionTestCase, TransactionType,
};

pub(crate) mod macros;

/// A [`TestRunner`] with a [`BasicKernel`].
pub type TestRunner<RT, S> = TestRunnerWithKernel<RT, BasicKernel<S, MockDaSpec>, S>;

generate_optimistic_runtime!(TestOptimisticRuntime <= value_setter: ValueSetter<S>);

/// Utilities for generating genesis configs.
pub mod genesis;

/// Utilities for hooks relating to test runtimes.
pub mod hooks;
/// Traits used to define interfaces for the runtime.
pub mod traits;
use traits::MinimalGenesis;

#[cfg(test)]
mod tests;

type DefaultSpecWithHasher<S> = DefaultStorageSpec<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>;

type NoncesMap<S> = HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64>;

/// Defines a slot receipt. A slot receipt is a list of [`BatchReceipt`]s and a block header.
pub struct SlotReceipt<S: Spec, Da: DaSpec> {
    batch_receipts: Vec<BatchReceipt<S, Da>>,
}

impl<S: Spec, Da: DaSpec> SlotReceipt<S, Da> {
    /// Returns the last batch receipt in the slot receipt.
    pub fn last_batch_receipt(&self) -> &BatchReceipt<S, Da> {
        self.batch_receipts.last().unwrap()
    }

    /// Returns the last transaction receipt in the last batch receipt of the slot receipt.
    pub fn last_tx_receipt(&self) -> &TransactionReceipt<S> {
        self.last_batch_receipt().tx_receipts.last().unwrap()
    }

    /// Returns the batch receipts contained in the slot receipt.
    pub fn batch_receipts(&self) -> &[BatchReceipt<S, Da>] {
        &self.batch_receipts
    }
}

/// TestRunner specific configuration values.
pub struct RunnerConfig<Da: DaSpec> {
    /// The sequencers DA address used as the address of the sender of a blob.
    pub sequencer_da_address: Da::Address,
    /// All blocks produced by the runner will be at the time provided.
    /// This is useful if your tests are dependent on timestamps.
    pub freeze_time: Option<Time>,
}

/// Stateful test runner that can be used to run and accumulate slot results for a given runtime.
pub struct TestRunnerWithKernel<
    RT: Runtime<S, MockDaSpec>,
    K: KernelSlotHooks<S, MockDaSpec>,
    S: Spec,
> {
    stf: StfBlueprint<S, MockDaSpec, RT, K>,
    nonces: HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    slot_receipts: Vec<SlotReceipt<S, MockDaSpec>>,
    state_root: <S::Storage as Storage>::Root,
    storage_manager: SimpleStorageManager<DefaultSpecWithHasher<S>>,
    /// Test runner configuration.
    pub config: RunnerConfig<MockDaSpec>,
}

/// The output of the runner
pub type TestApplySlotOutput<RT, S> =
    TestApplySlotOutputWithKernel<RT, BasicKernel<S, MockDaSpec>, S>;

type TestApplySlotOutputWithKernel<RT, K, S> = ApplySlotOutput<
    <S as Spec>::InnerZkvm,
    <S as Spec>::OuterZkvm,
    MockDaSpec,
    TestStfBlueprintWithKernel<RT, K, S>,
>;

/// The output of the runner
pub struct RunnerOutput<S: Spec> {
    /// The slot receipt emitted at the end of the slot execution
    pub receipt: SlotReceipt<S, MockDaSpec>,
    /// The change set containing the delta of the state after the slot execution
    pub change_set: NativeChangeSet,
    /// The root of the state after the slot execution
    pub root: <<S as Spec>::Storage as Storage>::Root,
}

impl<RT, K, S> TestRunnerWithKernel<RT, K, S>
where
    RT: Runtime<S, MockDaSpec> + MinimalGenesis<S, Da = MockDaSpec>,
    S: Spec<Storage = ProverStorage<DefaultSpecWithHasher<S>>>,
    K: KernelSlotHooks<S, MockDaSpec, BlobType = BlobDataWithId> + KernelWithSlotMapping<S>,
{
    /// Returns the runtime of the test runner.
    pub fn runtime(&self) -> &RT {
        self.stf.runtime()
    }

    /// Returns a reference to the storage manager of the test runner.
    pub fn storage_manager(&self) -> &SimpleStorageManager<DefaultSpecWithHasher<S>> {
        &self.storage_manager
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

    /// Returns the current virtual slot number.
    pub fn virtual_slot(&self) -> u64 {
        self.query_kernel_state(|kernel| kernel.virtual_slot_number())
    }

    /// A simple helper function to get the balance of a given address in the gas token currency with an [`InfallibleStateAccessor`].
    /// This can be used to check the balance of an address in closures.
    pub fn bank_gas_balance(
        address: &S::Address,
        state: &mut impl InfallibleStateAccessor,
    ) -> Option<u64> {
        sov_bank::Bank::<S>::default()
            .get_balance_of(address, config_gas_token_id(), state)
            .unwrap_infallible()
    }

    /// Returns the slot receipts accumulated by the state runner
    pub fn receipts(&self) -> &Vec<SlotReceipt<S, MockDaSpec>> {
        &self.slot_receipts
    }

    /// Returns a reference to the nonces used by the state runner
    pub fn nonces(&self) -> &HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64> {
        &self.nonces
    }

    fn current_state(&self) -> ApiStateAccessor<S> {
        let stf_state = self.storage_manager.create_storage();
        let kernel = K::default();

        let mut state_checkpoint =
            StateCheckpoint::<S::Storage>::new(stf_state.clone(), self.stf.kernel());

        let base_fee_per_gas = kernel.base_fee_per_gas(&mut state_checkpoint);

        ApiStateAccessor::<S>::new_with_price(
            &state_checkpoint,
            Arc::new(kernel),
            None,
            base_fee_per_gas,
        )
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
        &self,
        query: impl FnOnce(&mut ApiStateAccessor<S>) -> Output,
    ) -> Output {
        query(&mut self.current_state())
    }

    /// TODO(@theochap): A temporary solution until `https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1192` is resolved.
    /// Updates the state of the rollup by committing the changes of the given closure.
    pub fn __apply_to_state(&mut self, query: impl FnOnce(&mut StateCheckpoint<S::Storage>)) {
        let stf_state = self.storage_manager.create_storage();

        let mut state = StateCheckpoint::<S::Storage>::new(stf_state.clone(), self.stf.kernel());

        query(&mut state);

        let (reads_writes, _, witness) = state.freeze();

        let (_new_state_root, change_set) = stf_state
            .validate_and_materialize(reads_writes, &witness)
            .unwrap();

        self.storage_manager.commit(change_set);
    }

    /// Allows to query the current kernel state.
    pub fn query_kernel_state<Output>(
        &self,
        query: impl FnOnce(&mut KernelStateAccessor<S::Storage>) -> Output,
    ) -> Output {
        let stf_state = self.storage_manager.create_storage();

        let state = &mut StateCheckpoint::new(stf_state, self.stf.kernel());

        let mut kernel_accessor = self.stf.kernel().accessor(state);

        query(&mut kernel_accessor)
    }

    /// Builds a new test runner and runs genesis.
    pub fn new_with_genesis(
        genesis_config: GenesisParams<<RT as Genesis>::Config, K::GenesisConfig>,
        runtime: RT,
    ) -> Self {
        // Use the runtime to create an STF blueprint
        let stf = StfBlueprint::<S, MockDaSpec, RT, K>::with_runtime(runtime);

        // ----- Setup and run genesis ---------
        let temp_dir = tempfile::tempdir().unwrap();
        let mut storage_manager = SimpleStorageManager::new(temp_dir.path());

        let sequencer_da_address =
            <RT as MinimalGenesis<S>>::sequencer_registry_config(&genesis_config.runtime)
                .seq_da_address;

        let stf_state = storage_manager.create_storage();
        let (state_root, change_set) = stf.init_chain(stf_state, genesis_config);

        storage_manager.commit(change_set);

        // ----- End genesis ---------

        let config = RunnerConfig {
            sequencer_da_address,
            freeze_time: None,
        };

        Self {
            nonces: HashMap::new(),
            slot_receipts: Vec::new(),
            state_root,
            storage_manager,
            stf,
            config,
        }
    }

    fn next_header(&mut self) -> MockBlockHeader {
        let height = self.curr_slot_number() + 1;
        if let Some(timestamp) = &self.config.freeze_time {
            MockBlockHeader::new(height, timestamp.clone())
        } else {
            MockBlockHeader::from_height(height)
        }
    }

    fn txs_to_blobs<M: Module>(
        txs: Vec<TransactionType<M, S>>,
        sequencer: <MockDaSpec as DaSpec>::Address,
        nonces: &mut HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
        state: &mut ApiStateAccessor<S>,
    ) -> RelevantBlobs<MockBlob>
    where
        RT: EncodeCall<M>,
    {
        Self::batches_to_blobs(vec![(BatchType(txs), sequencer)], nonces, state)
    }

    /// Builds [`RelevantBlobs`] from a list of [`BatchType`]s.
    ///
    /// Note: This should be used with a [`BasicKernel`] implementation.
    pub fn batches_to_blobs<M: Module>(
        batches: Vec<(BatchType<M, S>, MockAddress)>,
        nonces: &mut HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
        state: &mut ApiStateAccessor<S>,
    ) -> RelevantBlobs<MockBlob>
    where
        RT: EncodeCall<M>,
    {
        let blobs = batches
            .into_iter()
            .map(|(batch, sequencer)| {
                let txns = batch
                    .0
                    .into_iter()
                    .map(|tx| tx.to_serialized_authenticated_tx::<RT>(nonces, state))
                    .collect::<Vec<_>>();
                let batch = Batch::new(txns);
                MockBlob::new_with_hash(borsh::to_vec(&batch).unwrap(), sequencer)
            })
            .collect::<Vec<_>>();

        RelevantBlobs {
            batch_blobs: blobs,
            proof_blobs: vec![],
        }
    }

    /// Builds [`RelevantBlobs`] from a list of [`SoftConfirmationBlobInfo`]s.
    ///
    /// To be used in soft-confirmation mode, ie with a [`sov_kernels::soft_confirmations::SoftConfirmationsKernel`] implementation.
    pub fn soft_confirmation_batches_to_blobs<M: Module>(
        batches: Vec<SoftConfirmationBlobInfo<S, M>>,
        nonces: &mut HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
        state: &mut ApiStateAccessor<S>,
    ) -> RelevantBlobs<MockBlob>
    where
        RT: EncodeCall<M>,
    {
        let blobs = batches
            .into_iter()
            .map(
                |SoftConfirmationBlobInfo {
                     batch_type: batch,
                     sequencer_address,
                     sequencer_info,
                 }| {
                    let raw_txns = batch
                        .0
                        .into_iter()
                        .map(|tx| tx.to_serialized_authenticated_tx::<RT>(nonces, state))
                        .collect::<Vec<_>>();

                    let serialized_batch = match sequencer_info {
                        SequencerInfo::Preferred {
                            slots_to_advance,
                            sequence_number,
                        } => borsh::to_vec(&PreferredBatchData {
                            sequence_number,
                            data: Batch::new(raw_txns),
                            virtual_slots_to_advance: slots_to_advance as u8,
                        })
                        .unwrap(),
                        SequencerInfo::Regular => borsh::to_vec(&Batch::new(raw_txns)).unwrap(),
                    };

                    MockBlob::new_with_hash(serialized_batch, sequencer_address)
                },
            )
            .collect::<Vec<_>>();

        RelevantBlobs {
            batch_blobs: blobs,
            proof_blobs: vec![],
        }
    }

    /// Simulates execution of the provided input without committing to the updated state.
    /// This is useful to retreive non-deterministic outcomes associated with execution such as
    /// dynamic gas prices.
    pub fn simulate<T: Into<SlotInput<S, M>>, M>(
        &mut self,
        input: T,
    ) -> (TestApplySlotOutputWithKernel<RT, K, S>, NoncesMap<S>)
    where
        M: Module,
        RT: EncodeCall<M>,
    {
        let block_header = self.next_header();
        let stf_state = self.storage_manager.create_storage();
        let slot_input: SlotInput<S, M> = input.into();
        let sequencer = self.config.sequencer_da_address;
        let mut state = self.current_state();
        let mut nonces = self.nonces.clone();

        let mut blobs = match slot_input {
            SlotInput::Transaction(tx) => {
                Self::txs_to_blobs(vec![tx], sequencer, &mut nonces, &mut state)
            }
            SlotInput::Batch(batch) => {
                Self::txs_to_blobs(batch.0, sequencer, &mut nonces, &mut state)
            }
            SlotInput::Proof(proof) => {
                let blob = MockBlob::new_with_hash(proof.0, sequencer);

                RelevantBlobs {
                    batch_blobs: vec![],
                    proof_blobs: vec![blob],
                }
            }
            SlotInput::Blobs(blobs) => blobs,
        };
        (
            self.stf.apply_slot(
                self.state_root(),
                stf_state.clone(),
                Default::default(),
                &block_header,
                &Default::default(),
                blobs.as_iters(),
                ExecutionContext::Node, // We care more about testing the full node than the sequencer simulation
            ),
            nonces,
        )
    }

    /// Executes the provided input and commits the state updates.
    /// This is useful for executing setup transactions that aren't test cases.
    pub fn execute<T: Into<SlotInput<S, M>>, M>(
        &mut self,
        input: T,
    ) -> TestApplySlotOutputWithKernel<RT, K, S>
    where
        M: Module,
        RT: EncodeCall<M>,
    {
        let (result, nonces) = self.simulate(input);
        self.commit_apply_slot_output(&result, nonces);

        result
    }

    fn commit_apply_slot_output(
        &mut self,
        output: &TestApplySlotOutputWithKernel<RT, K, S>,
        nonces: NoncesMap<S>,
    ) {
        self.storage_manager.commit(output.change_set.clone());
        self.state_root = output.state_root;
        self.slot_receipts.push(SlotReceipt {
            batch_receipts: output.batch_receipts.clone(),
        });
        self.nonces = nonces;
    }

    /// Advance the rollup `slots_to_advance` slots without executing
    /// any transactions.
    pub fn advance_slots(&mut self, slots_to_advance: usize) -> &mut Self {
        for _ in 0..slots_to_advance {
            let block_header = self.next_header();
            let stf_state = self.storage_manager.create_storage();
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
                ExecutionContext::Node,
            );
            self.commit_apply_slot_output(&result, self.nonces.clone());
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
        let result = self.execute(transaction_test.input);
        let batch_receipt = result.batch_receipts[0].clone();
        let tx_receipt = batch_receipt.tx_receipts[0].clone();
        let gas_used = get_gas_used(&tx_receipt);
        let gas_price = batch_receipt.gas_price.clone();

        let ctx = TransactionAssertContext::from_receipt::<MockDaSpec>(
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
        let result = self.execute(batch_test.input);
        let ctx = BatchAssertContext {
            sender_da_address: self.config.sequencer_da_address,
            batch_receipt: result.batch_receipts.first().cloned(),
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
        let result = self.execute(proof_test.input);
        let proof_receipt = result.proof_receipts.first().cloned();

        let gas_value_used = if let Some(proof_receipt) = &proof_receipt {
            let gas_used = <S as Spec>::Gas::try_from(proof_receipt.gas_used.clone()).unwrap_or_else(
                |_|
                panic!(
                    "Impossible to convert gas used {:?} to a gas unit {}. This is a bug - the batch receipt should always contain the correct number of gas dimensions. Please report this bug",
                    proof_receipt.gas_used,
                    std::any::type_name::<S::Gas>()
                )
            );
            let gas_price = <<S as Spec>::Gas as sov_modules_api::Gas>::Price::try_from(
                proof_receipt.gas_price.clone(),
            )
            .unwrap_or_else(
                |_|
                panic!(
                    "Impossible to convert gas used {:?} to a gas unit {}. This is a bug - the batch receipt should always contain the correct number of gas dimensions. Please report this bug",
                    proof_receipt.gas_used,
                    std::any::type_name::<<S::Gas as Gas>::Price>()
                )
            );

            gas_used.value(&gas_price)
        } else {
            0
        };

        let ctx = ProofAssertContext {
            proof_receipt,
            gas_value_used,
        };
        (proof_test.assert)(ctx, &mut self.current_state());

        self
    }
}

/// Assert that a transaction reverted for the expected reason.
pub fn assert_tx_reverted_with_reason<S: Spec>(
    result: TxEffect<TxReceiptContents<S>>,
    reason: anyhow::Error,
) {
    if let TxEffect::Reverted(contents) = result {
        assert_eq!(
            &contents.reason,
            &Error::ModuleError(reason),
            "The transaction should have reverted because instead the outcome was {:?}",
            contents
        );
    } else {
        panic!(
            "The transaction should have reverted because {}, instead the outcome was {:?}",
            reason, result
        );
    }
}
