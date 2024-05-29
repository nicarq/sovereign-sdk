use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use sov_attester_incentives::AttesterIncentives;
use sov_bank::{Bank, IntoPayable};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::capabilities::{
    AuthorizeSequencerError, GasEnforcer, HasCapabilities, RawTx, RuntimeAuthenticator,
    RuntimeAuthorization, SequencerAuthorization, TryReserveGasError,
};
use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::transaction::{AuthenticatedTransactionData, TransactionConsumption};
use sov_modules_api::{
    AuthenticationResult, AuthorizationData, Context, DispatchCall, EncodeCall, Gas, Genesis,
    GenesisState, Module, ModuleInfo, PreExecWorkingSet, RuntimeEventProcessor, Spec,
    StateCheckpoint, TxScratchpad, TypedEvent, WorkingSet,
};
use sov_modules_stf_blueprint::{BatchSequencerOutcome, Runtime};
use sov_rollup_interface::da::DaSpec;
use sov_sequencer_registry::{SequencerRegistry, SequencerStakeMeter};

use super::traits::{MinimalRuntime, StandardRuntime, TestRuntimeHookOverrides};

pub(super) type WorkingSetClosure<T> = Box<dyn FnOnce(&mut <T as TxHooks>::TxState) + Send + Sync>;

/// A queue of closures which can be executed in a `Runtime`'s post transaction hook.
#[derive(Default)]
pub(crate) struct ClosureQueue<T: TxHooks> {
    closures: Mutex<VecDeque<WorkingSetClosure<T>>>,
}

impl<RT: TxHooks> ClosureQueue<RT> {
    pub fn insert_all(&self, closures: Vec<WorkingSetClosure<RT>>) {
        // Sleep until the the queue is empty. This ensures that two different tests using the same runtime
        // cannot pollute each other's closure queues. Note that this requires a catch_unwind handler when a test panics
        // to empty the queue so that other tests can run.
        let mut contents = self.closures.lock().unwrap();
        contents.extend(closures);
    }

    pub fn try_get_next(&self) -> Option<WorkingSetClosure<RT>> {
        self.closures.lock().unwrap().pop_front()
    }
}

#[derive(Default, Clone)]
pub struct TestRuntimeWrapper<S: Spec, Da: DaSpec, T: StandardRuntime<S, Da>> {
    pub inner: T,
    pub(super) hook_action_queue: Arc<ClosureQueue<T>>,
    phantom: PhantomData<(S, Da)>,
}

impl<S, Da, T> TxHooks for TestRuntimeWrapper<S, Da, T>
where
    Self: TestRuntimeHookOverrides<S, Da>,
    T: StandardRuntime<S, Da>,
    S: Spec,
    Da: DaSpec,
{
    type Spec = S;
    type TxState = WorkingSet<S>;

    fn pre_dispatch_tx_hook(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        working_set: &mut Self::TxState,
    ) -> anyhow::Result<()> {
        self.pre_dispatch_tx_hook_override(tx, working_set)
    }

    fn post_dispatch_tx_hook(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        ctx: &Context<S>,
        working_set: &mut Self::TxState,
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

    type AuthorizationData = AuthorizationData<S>;

    fn authenticate(
        &self,
        raw_tx: &RawTx,
        pre_exec_ws: &mut PreExecWorkingSet<S, Self::SequencerStakeMeter>,
    ) -> AuthenticationResult<S, Self::Decodable, Self::AuthorizationData> {
        sov_modules_api::authenticate::<S, Self, Self::SequencerStakeMeter>(
            &raw_tx.data,
            pre_exec_ws,
        )
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

    fn endpoints(
        _storage: tokio::sync::watch::Receiver<S::Storage>,
    ) -> sov_modules_stf_blueprint::RuntimeEndpoints {
        todo!()
    }

    fn genesis_config(
        _genesis_paths: &Self::GenesisPaths,
    ) -> Result<Self::GenesisConfig, anyhow::Error> {
        todo!()
    }
}

// This test runtime has custom implementations of the capabilities
impl<S: Spec, Da: DaSpec, T: StandardRuntime<S, Da>> HasCapabilities<S, Da>
    for TestRuntimeWrapper<S, Da, T>
{
    type Capabilities<'a> = Self
    where
    T: 'a,;
    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    type AuthorizationData = AuthorizationData<S>;

    fn capabilities(&self) -> Self::Capabilities<'_> {
        Self::default()
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
        pre_exec_working_set: PreExecWorkingSet<S, Self::PreExecChecksMeter>,
    ) -> Result<WorkingSet<S>, TryReserveGasError<S, Self::PreExecChecksMeter>> {
        self.bank()
            .reserve_gas(tx, context.sender(), pre_exec_working_set)
            .map_err(Into::into)
    }

    fn allocate_consumed_gas(
        &self,
        tx_consumption: &TransactionConsumption<S::Gas>,
        tx_scratchpad: &mut TxScratchpad<S>,
    ) {
        self.bank().allocate_consumed_gas(
            &self.attester_incentives().id().to_payable(),
            &self.sequencer_registry().id().to_payable(),
            tx_consumption,
            tx_scratchpad,
        );
    }

    fn refund_remaining_gas(
        &self,
        context: &Context<S>,
        tx_consumption: &TransactionConsumption<S::Gas>,
        tx_scratchpad: &mut TxScratchpad<S>,
    ) {
        self.bank()
            .refund_remaining_gas(context.sender(), tx_consumption, tx_scratchpad);
    }
}

impl<S: Spec, Da: DaSpec, T: StandardRuntime<S, Da>> SequencerAuthorization<S, Da>
    for TestRuntimeWrapper<S, Da, T>
{
    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    fn authorize_sequencer(
        &self,
        sequencer: &<Da as DaSpec>::Address,
        base_fee_per_gas: &<S::Gas as Gas>::Price,
        tx_scratchpad: TxScratchpad<S>,
    ) -> Result<PreExecWorkingSet<S, Self::SequencerStakeMeter>, AuthorizeSequencerError<S>> {
        self.sequencer_registry()
            .authorize_sequencer(sequencer, base_fee_per_gas, tx_scratchpad)
    }

    fn penalize_sequencer(
        &self,
        sequencer: &Da::Address,
        pre_exec_working_set: PreExecWorkingSet<S, Self::SequencerStakeMeter>,
    ) -> TxScratchpad<S> {
        self.sequencer_registry()
            .penalize_sequencer(sequencer, pre_exec_working_set)
    }
}

impl<T: StandardRuntime<S, Da>, S: Spec, Da: DaSpec> RuntimeAuthorization<S, Da>
    for TestRuntimeWrapper<S, Da, T>
{
    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    type AuthorizationData = AuthorizationData<S>;
    /// Prevents duplicate transactions from running.
    // TODO(@preston-evans98): Use type system to prevent writing to the `StateCheckpoint` during this check
    fn check_uniqueness(
        &self,
        _auth_tx: &Self::AuthorizationData,
        _context: &Context<S>,
        _working_set: &mut PreExecWorkingSet<S, Self::SequencerStakeMeter>,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Resolves the context for a transaction.
    fn resolve_context(
        &self,
        auth_tx: &Self::AuthorizationData,
        sequencer: &Da::Address,
        height: u64,
        working_set: &mut PreExecWorkingSet<S, Self::SequencerStakeMeter>,
    ) -> Result<Context<S>, anyhow::Error> {
        let sender = auth_tx.default_address.clone().unwrap();
        let sequencer = self
            .sequencer_registry()
            .resolve_da_address(sequencer, working_set)
            .expect("Sequencer is no longer registered by the time of context resolution. This is a bug");
        Ok(Context::new(
            sender,
            auth_tx.credentials.clone(),
            sequencer,
            height,
        ))
    }

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        _auth_tx: &Self::AuthorizationData,
        _sequencer: &Da::Address,
        _tx_scratchpad: &mut TxScratchpad<S>,
    ) {
    }
}
