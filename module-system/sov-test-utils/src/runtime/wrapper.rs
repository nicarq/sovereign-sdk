use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use sov_attester_incentives::AttesterIncentives;
use sov_bank::{Bank, IntoPayable, ReserveGasError};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::capabilities::{
    AuthenticationError, GasEnforcer, RawTx, RuntimeAuthenticator, RuntimeAuthorization,
    SequencerAuthorization,
};
use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{
    Context, DispatchCall, EncodeCall, Gas, Genesis, GenesisState, Module, ModuleInfo,
    RuntimeEventProcessor, Spec, StateCheckpoint, TransactionConsumption, TypedEvent, WorkingSet,
};
use sov_modules_stf_blueprint::{BatchSequencerOutcome, Runtime};
use sov_rollup_interface::da::DaSpec;
use sov_sequencer_registry::{SequencerRegistry, SequencerStakeMeter};

use super::traits::{MinimalRuntime, StandardRuntime, TestRuntimeHookOverrides};
use crate::runtime::AuthenticatedTransactionAndRawHash;

pub(super) type WorkingSetClosure<S> = Box<dyn FnOnce(&mut WorkingSet<S>) + Send>;

/// A queue of closures which can be executed in a `Runtime`'s post transaction hook.
#[derive(Default)]
pub(crate) struct ClosureQueue<S: Spec> {
    closures: Mutex<VecDeque<WorkingSetClosure<S>>>,
}

impl<S: Spec> ClosureQueue<S> {
    pub fn insert_all(&self, closures: Vec<WorkingSetClosure<S>>) {
        // Sleep until the the queue is empty. This ensures that two different tests using the same runtime
        // cannot pollute each other's closure queues. Note that this requires a catch_unwind handler when a test panics
        // to empty the queue so that other tests can run.
        let mut contents = self.closures.lock().unwrap();
        contents.extend(closures);
    }

    pub fn try_get_next(&self) -> Option<WorkingSetClosure<S>> {
        self.closures.lock().unwrap().pop_front()
    }
}

#[derive(Default, Clone)]
pub struct TestRuntimeWrapper<S: Spec, Da: DaSpec, T: StandardRuntime<S, Da>> {
    pub inner: T,
    pub(super) hook_action_queue: Arc<ClosureQueue<S>>,
    phantom: PhantomData<Da>,
}

impl<S, Da, T> TxHooks for TestRuntimeWrapper<S, Da, T>
where
    Self: TestRuntimeHookOverrides<S, Da>,
    T: StandardRuntime<S, Da>,
    S: Spec,
    Da: DaSpec,
{
    type Spec = S;

    fn pre_dispatch_tx_hook(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<()> {
        self.pre_dispatch_tx_hook_override(tx, working_set)
    }

    fn post_dispatch_tx_hook(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        ctx: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<()> {
        self.post_dispatch_tx_hook_override(tx, ctx, working_set)
    }
}

impl<S, Da: DaSpec, T> DispatchCall for TestRuntimeWrapper<S, Da, T>
where
    T: StandardRuntime<S, Da>,
    S: Spec,
    Da: DaSpec,
{
    type Spec = S;

    type Decodable = T::Decodable;

    fn decode_call(serialized_message: &[u8]) -> Result<Self::Decodable, std::io::Error> {
        T::decode_call(serialized_message)
    }

    fn dispatch_call(
        &self,
        message: Self::Decodable,
        working_set: &mut WorkingSet<S>,
        context: &Context<S>,
    ) -> Result<sov_modules_api::CallResponse, sov_modules_api::Error> {
        self.inner.dispatch_call(message, working_set, context)
    }

    fn module_id(&self, message: &Self::Decodable) -> &sov_modules_api::ModuleId {
        self.inner.module_id(message)
    }
}

impl<S, Da: DaSpec, T> ApplyBatchHooks<Da> for TestRuntimeWrapper<S, Da, T>
where
    Self: TestRuntimeHookOverrides<S, Da>,
    T: StandardRuntime<S, Da>,
    S: Spec,
    T: MinimalRuntime<S, Da>,
{
    type Spec = S;
    type BatchResult = BatchSequencerOutcome;

    fn begin_batch_hook(
        &self,
        batch: &mut BatchWithId,
        sender: &Da::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> anyhow::Result<()> {
        self.begin_batch_hook_override(batch, sender, state_checkpoint)
    }

    fn end_batch_hook(
        &self,
        result: Self::BatchResult,
        sender: &Da::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.end_batch_hook_override(result, sender, state_checkpoint);
    }
}

impl<S, Da, T> SlotHooks for TestRuntimeWrapper<S, Da, T>
where
    Self: TestRuntimeHookOverrides<S, Da>,
    T: StandardRuntime<S, Da>,
    S: Spec,
    Da: DaSpec,
{
    type Spec = S;

    fn begin_slot_hook(
        &self,
        pre_state_root: S::VisibleHash,
        working_set: &mut sov_modules_api::VersionedStateReadWriter<StateCheckpoint<S>>,
    ) {
        self.begin_slot_hook_override(pre_state_root, working_set);
    }

    fn end_slot_hook(&self, working_set: &mut StateCheckpoint<S>) {
        self.end_slot_hook_override(working_set);
    }
}

impl<S, Da, T> FinalizeHook for TestRuntimeWrapper<S, Da, T>
where
    Self: TestRuntimeHookOverrides<S, Da>,
    T: StandardRuntime<S, Da>,
    S: Spec,
    Da: DaSpec,
{
    type Spec = S;
    fn finalize_hook(
        &self,
        root_hash: S::VisibleHash,
        accessory_working_set: &mut impl sov_modules_api::prelude::StateReaderAndWriter<
            sov_state::namespaces::Accessory,
        >,
    ) {
        self.finalize_hook_override(root_hash, accessory_working_set);
    }
}

impl<S: Spec, Da: DaSpec, T: StandardRuntime<S, Da>> RuntimeAuthenticator<S>
    for TestRuntimeWrapper<S, Da, T>
{
    type Decodable = <T as DispatchCall>::Decodable;

    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    fn authenticate(
        &self,
        raw_tx: &RawTx,
        sequencer_stake_meter: &mut Self::SequencerStakeMeter,
    ) -> Result<(AuthenticatedTransactionAndRawHash<S>, Self::Decodable), AuthenticationError> {
        sov_modules_api::authenticate::<S, Self>(&raw_tx.data, sequencer_stake_meter)
    }
}

impl<S: Spec, Da: DaSpec, T: StandardRuntime<S, Da>> MinimalRuntime<S, Da>
    for TestRuntimeWrapper<S, Da, T>
{
    fn bank(&self) -> &Bank<S> {
        self.inner.bank()
    }

    fn sequencer_registry(&self) -> &SequencerRegistry<S, Da> {
        self.inner.sequencer_registry()
    }

    fn attester_incentives(&self) -> &AttesterIncentives<S, Da> {
        self.inner.attester_incentives()
    }
}

impl<S, Da, T, M> EncodeCall<M> for TestRuntimeWrapper<S, Da, T>
where
    T: EncodeCall<M>,
    T: StandardRuntime<S, Da>,
    S: Spec,
    Da: DaSpec,
    M: Module,
{
    fn encode_call(message: M::CallMessage) -> Vec<u8> {
        T::encode_call(message)
    }
}

impl<S, Da, T> Runtime<S, Da> for TestRuntimeWrapper<S, Da, T>
where
    Self: TestRuntimeHookOverrides<S, Da>,
    T: StandardRuntime<S, Da>,
    Self: DispatchCall<
        Decodable = <Self as RuntimeAuthenticator<S>>::Decodable,
        Spec = <T as DispatchCall>::Spec,
    >,
    <Self as Genesis>::Config: Send + Sync,
    S: Spec,
    Da: DaSpec,
{
    type GenesisConfig = <Self as Genesis>::Config;

    type GenesisPaths = ();

    fn rpc_methods(_storage: tokio::sync::watch::Receiver<S::Storage>) -> jsonrpsee::RpcModule<()> {
        todo!()
    }

    fn genesis_config(
        _genesis_paths: &Self::GenesisPaths,
    ) -> Result<Self::GenesisConfig, anyhow::Error> {
        todo!()
    }
}

impl<S: Spec, Da: DaSpec, T: StandardRuntime<S, Da>> RuntimeEventProcessor
    for TestRuntimeWrapper<S, Da, T>
{
    type RuntimeEvent = T::RuntimeEvent;
    fn convert_to_runtime_event(event: TypedEvent) -> Option<Self::RuntimeEvent> {
        T::convert_to_runtime_event(event)
    }
}

impl<S: Spec, Da: DaSpec, T: StandardRuntime<S, Da>> Genesis for TestRuntimeWrapper<S, Da, T> {
    type Spec = S;
    type Config = T::Config;

    fn genesis(
        &self,
        config: &Self::Config,
        working_set: &mut impl GenesisState<S>,
    ) -> Result<(), sov_modules_api::Error> {
        self.inner.genesis(config, working_set)
    }
}

impl<S: Spec, Da: DaSpec, T: StandardRuntime<S, Da>> GasEnforcer<S, Da>
    for TestRuntimeWrapper<S, Da, T>
{
    /// A type that tracks the gas consumed by pre-execution checks
    type PreExecChecksMeter = SequencerStakeMeter<S::Gas>;

    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        context: &Context<S>,
        gas_price: &<S::Gas as Gas>::Price,
        pre_exec_checks_meter: &Self::PreExecChecksMeter,
        state_checkpoint: StateCheckpoint<S>,
    ) -> Result<WorkingSet<S>, StateCheckpoint<S>> {
        self.inner
            .bank()
            .reserve_gas(
                tx,
                gas_price,
                context.sender(),
                pre_exec_checks_meter,
                state_checkpoint,
            )
            .map_err(
                |ReserveGasError {
                     state_checkpoint,
                     reason,
                 }| {
                    tracing::debug!(
                        "Unable to reserve gas from {}. {}",
                        reason,
                        context.sender()
                    );
                    state_checkpoint
                },
            )
    }

    fn allocate_consumed_gas(
        &self,
        consumption: &TransactionConsumption<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.inner.bank().allocate_consumed_gas(
            &self.attester_incentives().id().to_payable(),
            &self.sequencer_registry().id().to_payable(),
            consumption,
            state_checkpoint,
        );
    }

    /// Refunds any remaining gas to the payer after the transaction is processed.
    fn refund_remaining_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        context: &Context<S>,
        consumption: &TransactionConsumption<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.inner
            .bank()
            .refund_remaining_gas(tx, context.sender(), consumption, state_checkpoint);
    }
}

impl<S: Spec, Da: DaSpec, T: StandardRuntime<S, Da>> SequencerAuthorization<S, Da>
    for TestRuntimeWrapper<S, Da, T>
{
    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    fn authorize_sequencer(
        &self,
        sequencer: &Da::Address,
        base_fee_per_gas: &<S::Gas as Gas>::Price,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<Self::SequencerStakeMeter, anyhow::Error> {
        self.inner
            .sequencer_registry()
            .authorize_sequencer(sequencer, base_fee_per_gas, state_checkpoint)
            .map_err(|e| {
                anyhow::anyhow!("An error occurred while checking the sequencer bond: {e}")
            })
    }

    fn refund_sequencer(
        &self,
        sequencer_stake_meter: &mut Self::SequencerStakeMeter,
        refund_amount: u64,
    ) {
        self.inner
            .sequencer_registry()
            .refund_sequencer(sequencer_stake_meter, refund_amount);
    }

    fn penalize_sequencer(
        &self,
        sequencer: &Da::Address,
        sequencer_stake_meter: Self::SequencerStakeMeter,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.inner.sequencer_registry().penalize_sequencer(
            sequencer,
            sequencer_stake_meter,
            state_checkpoint,
        );
    }
}

impl<T: StandardRuntime<S, Da>, S: Spec, Da: DaSpec> RuntimeAuthorization<S, Da>
    for TestRuntimeWrapper<S, Da, T>
{
    /// Prevents duplicate transactions from running.
    // TODO(@preston-evans98): Use type system to prevent writing to the `StateCheckpoint` during this check
    fn check_uniqueness(
        &self,
        _tx: &AuthenticatedTransactionData<S>,
        _context: &Context<S>,
        _state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        _tx: &AuthenticatedTransactionData<S>,
        _sequencer: &Da::Address,
        _state_checkpoint: &mut StateCheckpoint<S>,
    ) {
    }

    /// Resolves the context for a transaction.
    fn resolve_context(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        sequencer: &Da::Address,
        height: u64,
        working_set: &mut StateCheckpoint<S>,
    ) -> Result<Context<S>, anyhow::Error> {
        let sender = tx.default_address.clone().unwrap();
        let sequencer = self
            .sequencer_registry()
            .resolve_da_address(sequencer, working_set)
            .expect("Sequencer is no longer registered by the time of context resolution. This is a bug");
        Ok(Context::new(
            sender,
            tx.credentials.clone(),
            sequencer,
            height,
        ))
    }
}
