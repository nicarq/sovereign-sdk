use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use anyhow::Context;
use axum::http::StatusCode;
use sov_modules_api::capabilities::{
    BlobSelector, BlobSelectorOutput, ChainState, FatalError, RollupHeight,
    TransactionAuthenticator,
};
use sov_modules_api::macros::config_value;
use sov_modules_api::{
    call_message_repr, BlobDataWithId, ChangeSet, DaSpec, ExecutionContext, FullyBakedTx, Gas,
    GasSpec, HexString, KernelStateAccessor, NoOpControlFlow, RejectReason, Runtime,
    RuntimeEventProcessor, RuntimeEventResponse, SelectedBlob, Spec, StateCheckpoint,
    StateUpdateInfo, TransactionReceipt, TxChangeSet, VersionReader, VisibleSlotNumber,
};
use sov_modules_stf_blueprint::{BatchReceipt, StfBlueprint};
use sov_rest_utils::{json_obj, ErrorObject};
use sov_rollup_interface::common::SlotNumber;
use sov_state::{Namespace, StateAccesses, StateRoot, Storage};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{self, Sender};
use tokio::sync::{broadcast, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{trace, warn};
use uuid::Uuid;

use super::state_root_compute::StateRootComputeRequest;
use super::{PreferredBatchToReplay, PreferredSequencerConfig, VisibleSlotNumberIncrease};
use crate::common::generic_accept_tx_error;
use crate::preferred::async_batch::MaybeAsyncBatch;
use crate::{SequencerConfig, SequencerEvent};

type TxReceiptWithEvents<S, Rt> = (
    TransactionReceipt<S>,
    Vec<RuntimeEventResponse<<Rt as RuntimeEventProcessor>::RuntimeEvent>>,
);

type BlockExecutionOutput<S> = (
    Vec<BatchReceipt<S>>,
    ChangeSet,
    StateAccesses,
    <<S as Spec>::Storage as Storage>::Witness,
);

#[derive(thiserror::Error, Debug)]
pub(crate) enum RollupBlockExecutorError<S: Spec> {
    #[error(transparent)]
    DecodeCall(#[from] FatalError),
    #[error("The sequencer is temporarily overloaded. Try again in a few seconds")]
    Overloaded,
    #[error("The transaction was rejected")]
    Rejected { reason: RejectReason, call: String },
    #[error("The transaction execution was unsuccessful")]
    UnsuccessfulTransaction { receipt: TransactionReceipt<S> },
}

impl<S: Spec> RollupBlockExecutorError<S> {
    pub fn into_http_error(self) -> ErrorObject {
        match self {
            RollupBlockExecutorError::DecodeCall(_) => ErrorObject {
                status: StatusCode::BAD_REQUEST,
                title: "Malformed transaction".to_string(),
                details: json_obj!({
                    "message": self.to_string(),
                }),
            },
            RollupBlockExecutorError::Overloaded => ErrorObject {
                status: StatusCode::SERVICE_UNAVAILABLE,
                title: "Temporarily unavailable".to_string(),
                details: json_obj!({
                    "message": self.to_string(),
                }),
            },
            RollupBlockExecutorError::Rejected { reason, call } => {
                reject_reason_to_error(reason, call)
            }
            RollupBlockExecutorError::UnsuccessfulTransaction { receipt } => {
                generic_accept_tx_error(receipt)
            }
        }
    }
}

type StateRootReceiver<S> =
    oneshot::Receiver<(RollupHeight, <<S as Spec>::Storage as Storage>::Root)>;

pub(crate) type EventCache<E> = Arc<tokio::sync::RwLock<BTreeMap<u64, (E, SlotNumber)>>>;

pub struct RollupBlockExecutor<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    pub checkpoint: StateCheckpoint<S>,
    rollup_block_task_state: Option<BackgroundTaskState<S>>,

    next_event_number: u64,
    events_sender: Option<broadcast::Sender<SequencerEvent<Rt>>>,
    config: SequencerConfig<S::Da, S::Address, PreferredSequencerConfig>,
    // A sender notifying that this acceptor has successfully shut down. We give a handle to
    // each background task when it is spawned, ensuring that this channel remains open as long
    // as any background task is operational even if the acceptor is dropped.
    shutdown_notifier: Sender<()>,
    state_root_request_sender: tokio::sync::mpsc::Sender<StateRootComputeRequest<S>>,
    state_roots: BTreeMap<RollupHeight, <S::Storage as Storage>::Root>,
    state_root_responses: VecDeque<StateRootReceiver<S>>,
    shutdown_receiver: watch::Receiver<()>,
    id: Uuid,
    cached_events: EventCache<RuntimeEventResponse<Rt::RuntimeEvent>>,
}

impl<S: Spec, Rt: Runtime<S>> RollupBlockExecutor<S, Rt> {
    /// The maximum number of transactions that can be buffered before incoming txs start getting
    /// rejected.
    const MAX_BUFFERED_TXS: usize = 1;

    pub fn new(
        info: StateUpdateInfo<S::Storage>,
        events_sender: Option<broadcast::Sender<SequencerEvent<Rt>>>,
        config: SequencerConfig<S::Da, S::Address, PreferredSequencerConfig>,
        shutdown_notifier: Sender<()>,
        state_root_request_sender: tokio::sync::mpsc::Sender<StateRootComputeRequest<S>>,
        shutdown_receiver: watch::Receiver<()>,
        cached_events: EventCache<RuntimeEventResponse<Rt::RuntimeEvent>>,
    ) -> RollupBlockExecutor<S, Rt> {
        let mut rt = Rt::default();
        let checkpoint = StateCheckpoint::new(info.storage.clone(), &rt.kernel());

        RollupBlockExecutor {
            checkpoint,
            rollup_block_task_state: None,
            next_event_number: info.next_event_number,
            events_sender,
            config,
            shutdown_notifier,
            state_root_request_sender,
            state_roots: Default::default(),
            state_root_responses: Default::default(),
            id: Uuid::now_v7(),
            shutdown_receiver,
            cached_events,
        }
    }

    pub fn has_in_progress_batch(&self) -> bool {
        self.rollup_block_task_state.is_some()
    }

    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn replace_state(&mut self, other: Self) {
        if self.shutdown_receiver.has_changed().unwrap_or(true) {
            return;
        }

        tracing::trace!(
            "Replacing state for executor {} with executor {}",
            self.id,
            other.id
        );

        if let Some(task_state) = self.rollup_block_task_state.take() {
            task_state.shutdown().abort();
        }

        self.checkpoint = other.checkpoint;
        self.rollup_block_task_state = other.rollup_block_task_state;
        self.next_event_number = other.next_event_number;

        // Update our list of state roots from the other executor.
        self.state_roots = other.state_roots;
        self.state_root_responses = other.state_root_responses;
    }

    /// Calls to this method must happen "between"
    /// [`Self::start_rollup_block`] and
    /// [`Self::end_rollup_block`].
    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn apply_tx_to_in_progress_batch(
        &mut self,
        baked_tx: &FullyBakedTx,
    ) -> Result<TxReceiptWithEvents<S, Rt>, RollupBlockExecutorError<S>> {
        let result = self.apply_tx_to_in_progress_batch_inner(baked_tx).await;

        let slot_num = self.checkpoint.current_visible_slot_number();

        match result {
            Ok(r) => {
                let events = self.process_tx_receipt(&r, *slot_num).await;
                Ok((r, events))
            }
            Err(e) => Err(e),
        }
    }

    async fn apply_tx_to_in_progress_batch_inner(
        &mut self,
        baked_tx: &FullyBakedTx,
    ) -> Result<TransactionReceipt<S>, RollupBlockExecutorError<S>> {
        let Some(task_state) = self.rollup_block_task_state.as_mut() else {
            panic!("Accepting a transaction, yet there's no in-progress batch. This is a bug in the sequencer, please report it.");
        };

        let call = Rt::Auth::decode_serialized_tx(baked_tx)?;
        let call = Rt::wrap_call(call);

        if let Err(TrySendError::Full(_)) = task_state.tx_sender.try_send(baked_tx.clone()) {
            return Err(RollupBlockExecutorError::Overloaded);
        }

        let result = task_state
            .result_receiver
            .recv()
            .await
            .expect("The background task failed unexpectedly");

        let (receipt, change_set) =
            result.map_err(|reason| RollupBlockExecutorError::Rejected {
                reason,
                call: call_message_repr::<Rt>(&call),
            })?;

        if !receipt.receipt.is_successful() {
            return Err(RollupBlockExecutorError::UnsuccessfulTransaction { receipt });
        }

        self.checkpoint.apply_changes(change_set.0);

        Ok(receipt)
    }

    /// Returns true if [`super::db::PreferredSequencerDb::pop_tx`] ought to be called.
    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn replay_batch(
        &mut self,
        batch: &PreferredBatchToReplay,
        node_state_root: &<S::Storage as Storage>::Root,
    ) -> anyhow::Result<bool> {
        assert!(
            self.rollup_block_task_state.is_none(),
            "Replaying a preferred batch, but the state is invalid and doesn't allow it ({:?}). This is a bug, please report it.",
            self.rollup_block_task_state
        );

        trace!(
            num_txs = batch.batch.inner.data.len(),
            "Re-applying batch state changes"
        );

        self.start_rollup_block(
            batch.visible_slot_number_after_increase,
            batch.batch.inner.visible_slots_to_advance,
            node_state_root,
            // When replaying batches, we wish to be deterministic and not
            // filter out previously-accepted transactions simply because
            // they're not considered profitable enough based on the current
            // configuration value.
            //
            // TODO(@neysofu): write a test for this.
            // TODO(@neysofu): for the very last in-progress batch, this will
            // cause the rest of the batch to not have a minimum profit. We
            // might want to forcibly close that batch and start a new one, or
            // send the new configuration value over a channel.
            0,
        )
        .await;

        if self.shutdown_receiver.has_changed().unwrap_or(true) {
            return Ok(false);
        }
        trace!("Replaying txs");

        let last_tx_hash = batch.batch.tx_hashes.last();

        for (tx, tx_hash) in batch
            .batch
            .inner
            .data
            .iter()
            .zip(batch.batch.tx_hashes.iter())
        {
            trace!(
                %tx_hash,
                "Re-applying state changes for the soft-confirmed transaction"
            );

            if let Err(err) = self.apply_tx_to_in_progress_batch(tx).await {
                if Some(tx_hash) == last_tx_hash && batch.is_in_progress {
                    warn!(%tx_hash, "The very last transaction failed to be applied, this is likelythe result of a hard node crash. We'll remove it from the database and continue normal operations.");

                    return Ok(true);
                }

                tracing::error!(
                    "Transaction was soft-confirmed but failed to be re-applied; this is a bug, please report it {:?}",
                    err
                );
                std::process::exit(1);
            }
        }

        trace!("Done replaying txs");

        if !batch.is_in_progress {
            self.end_rollup_block().await;
        } else {
            trace!("The batch is still in progress; will keep the background task running");
        }

        Ok(false)
    }

    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn start_rollup_block(
        &mut self,
        sanity_check_visible_slot_number_after_increase: VisibleSlotNumber,
        visible_increase: VisibleSlotNumberIncrease,
        // We pass the node state root explicitly because retrieving it is
        // fallible, so it's convenient to front-load the error-checking.
        node_state_root: &<S::Storage as Storage>::Root,
        minimum_profit_per_tx: u128,
    ) {
        assert!(
            self.rollup_block_task_state.is_none(),
            "Starting a rollup block, but there's already one in progress {:?}. This is a bug, please report it.",
            self.rollup_block_task_state
        );

        // If we've started shutting down, don't start a new block.
        if self.shutdown_receiver.has_changed().unwrap_or(true) {
            tracing::info!("Shutdown receiver has changed. New block not started");
            return;
        }

        trace!(
            ?self.checkpoint,
            %visible_increase,
            "Beginning new rollup block and spawning background loop"
        );

        self.populate_state_roots(node_state_root).await;
        if self.shutdown_receiver.has_changed().unwrap_or(true) {
            tracing::info!("Shutdown receiver has changed. New block not started");
            return;
        }

        let old_visible_slot_number = self.checkpoint.current_visible_slot_number();
        let next_visible_slot_number = self
            .checkpoint
            .current_visible_slot_number()
            .advance(visible_increase.get().into());

        assert_eq!(
            next_visible_slot_number,
            sanity_check_visible_slot_number_after_increase,
            "Sanity check failed: visible slot number calculation was incorrect. This is a bug, please report it."
        );

        let (setup_sender, setup_receiver) = oneshot::channel();
        let (tx_sender, tx_receiver) = mpsc::channel(Self::MAX_BUFFERED_TXS);
        let (result_sender, result_receiver) = mpsc::channel(Self::MAX_BUFFERED_TXS);

        let handle = tokio::runtime::Handle::current().spawn_blocking({
            let ctx = RollupBlockTaskContext {
                checkpoint: self
                    .checkpoint
                    .clone_with_empty_witness_dropping_temp_cache(),
                tx_receiver,
                setup_sender,
                old_visible_slot_number,
                next_visible_slot_number,
                visible_increase,
                result_sender,
                shutdown_notifier: self.shutdown_notifier.clone(),
                old_rollup_height: self.checkpoint.rollup_height_to_access(),
                minimum_profit_per_tx,
                admin_addresses: self.config.admin_addresses.clone().into(),
                sequencer_rollup_address: self.config.rollup_address.clone(),
                sequencer_da_address: self.config.da_address.clone(),
            };

            move || rollup_block_task_body::<S, Rt>(ctx)
        });

        {
            // Wait for the background task to get up and running, and send the
            // initial change set.
            trace!("Applying setup changes...");
            let setup_changes = setup_receiver
                .await
                .with_context(|| "Setup must finish successfully")
                .expect(
                    "The sequencer can't recover from this error; this is a bug, please report it",
                );
            trace!("Applied setup changes");

            self.checkpoint.apply_changes(setup_changes);
            self.checkpoint
                .advance_visible_slot_number(visible_increase);
        }

        self.rollup_block_task_state = Some(BackgroundTaskState {
            handle,
            tx_sender,
            result_receiver,
        });
    }

    async fn process_tx_receipt(
        &mut self,
        tx_receipt: &TransactionReceipt<S>,
        current_slot_num: SlotNumber,
    ) -> Vec<RuntimeEventResponse<Rt::RuntimeEvent>> {
        let events = tx_receipt
            .events
            .iter()
            .zip(self.next_event_number..)
            .map(|(event, number)| {
                <RuntimeEventResponse<<Rt as RuntimeEventProcessor>::RuntimeEvent>>::try_from((
                    number, event,
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()
            .expect("Supposedly infallible conversion failed; this is a bug, please report it");

        self.next_event_number += events.len() as u64;

        if let Some(sender) = &self.events_sender {
            let mut cached_events = self.cached_events.write().await;
            cached_events.extend(
                events
                    .iter()
                    .cloned()
                    .map(|event| (event.number, (event, current_slot_num))),
            );
            for event in events.iter().cloned() {
                sender
                    .send(SequencerEvent {
                        tx_hash: tx_receipt.tx_hash,
                        event,
                    })
                    .ok();
            }
        }

        events
    }

    /// Before starting a rollup block, we need to have stored any visible state roots that it might need in state.
    /// In the node, this is done automatically, but sometimes the sequencer can run too far ahead of the node and need to compute these roots itself.
    async fn populate_state_roots(&mut self, node_state_root: &<S::Storage as Storage>::Root) {
        if self.shutdown_receiver.has_changed().unwrap_or(true) {
            return;
        }
        // If we don't have any state roots yet, insert the node's state root. That's our starting point.
        if self.state_roots.is_empty() {
            self.state_roots.insert(
                self.checkpoint.rollup_height_to_access(),
                node_state_root.clone(),
            );
        }

        // Compute the next visible root height that we need to fetch.
        let next_rollup_height = self.checkpoint.rollup_height_to_access().saturating_add(1);
        let next_visible_root_height =
            next_rollup_height.saturating_sub(config_value!("STATE_ROOT_DELAY_BLOCKS"));
        tracing::trace!(
            "Fetching state root for height: {} if necessary",
            next_visible_root_height
        );

        // If we don't have the next visible root locally, fetch it from the background task.
        // Note: The request will *always* be the next one in our inbound queue if we take this branch.
        // If this block is the first one computed using the RollupBlockExecutor, then the root we need is just the one from the node,
        // so we *don't* take this branch.
        // Otherwise, the request will already have been sent during the previous iteration of `end_rollup_block`, so we can just await it here.
        if next_visible_root_height
            > *self
                .state_roots
                .keys()
                .max()
                .unwrap_or(&RollupHeight::GENESIS)
        {
            tracing::trace!(
                "Fetching state root for height: {}",
                next_visible_root_height
            );
            let (received_height, next_visible_root) = match self.state_root_responses.pop_front().unwrap_or_else(||
                    panic!("Executor {} Needed response for state root for height {} before sending request. This is a bug in the `RollupBlockExecutor`, please report it.", self.id, next_visible_root_height))
            .await {
                Ok((received_height, next_visible_root)) => {
                   (received_height, next_visible_root)
                }
                Err(_) => {
                    tracing::info!("State root computation background task has shutdown. New block not started");
                    return;
                }
            };
            // Sanity check: the height we received should match the height we need.
            if received_height != next_visible_root_height {
                tracing::error!("Received height ({}) did not equal expected height for assertion {}. This is a bug in the RollupBlockExecutor, please report it.", received_height, next_visible_root_height);
                panic!("Received height ({}) did not equal expected height for assertion {}. This is a bug in the RollupBlockExecutor, please report it.", received_height, next_visible_root_height);
            }
            tracing::trace!(
                "Received state root for height {} : {}",
                next_visible_root_height,
                HexString(next_visible_root.namespace_root(sov_state::ProvableNamespace::User))
            );
            self.state_roots
                .insert(next_visible_root_height, next_visible_root);
        }
        // take all roots greater than self.started_from
        for (height, root) in self.state_roots.iter() {
            let user_root = root.namespace_root(sov_state::ProvableNamespace::User);
            let mut runtime = Rt::default();
            let mut kernel = runtime.kernel();
            let mut kernel_state =
                KernelStateAccessor::from_checkpoint(&kernel, &mut self.checkpoint);
            kernel.save_user_state_root(*height, user_root, &mut kernel_state);
        }
    }

    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn end_rollup_block(&mut self) {
        trace!("Ending rollup block");

        let task_state = self
            .rollup_block_task_state
            .take()
            .expect("No in-progress rollup block, nothing to do. This is a bug, please report it");

        let rollup_height = self.checkpoint.rollup_height_to_access();
        let (batch_receipts, changes, state_accesses, witness) =
            task_state.shutdown().await.expect(
                "Transaction acceptor task failed unexpectedly! This is a bug, please report it.",
            );

        for batch_receipt in batch_receipts {
            // We already increment the event number for our own transactions
            // inside `apply_tx_to_in_progress_batch`.
            if batch_receipt.inner.da_address == self.config.da_address {
                continue;
            }

            for tx_receipt in batch_receipt.tx_receipts {
                self.process_tx_receipt(
                    &tx_receipt,
                    *self.checkpoint.current_visible_slot_number(),
                )
                .await;
            }
        }

        tracing::trace!(executor_id = %self.id, "Sending state root computation to background task at height {}", rollup_height);
        let (response_channel, response_receiver) = oneshot::channel();
        self.state_root_responses.push_back(response_receiver);
        if self
            .state_root_request_sender
            .send(StateRootComputeRequest {
                state_accesses,
                witness,
                storage: self.checkpoint.storage().clone(),
                rollup_height,
                response_channel,
            })
            .await
            .is_err()
        {
            tracing::info!(executor_id = %self.id, "State root computation background task has shutdown. State root will not be computed.");
        }

        self.checkpoint.apply_changes(changes);

        trace!("Successfully ended rollup block");
    }
}

#[derive(Debug)]
struct BackgroundTaskState<S: Spec> {
    handle: JoinHandle<BlockExecutionOutput<S>>,
    tx_sender: mpsc::Sender<FullyBakedTx>,
    result_receiver: mpsc::Receiver<Result<(TransactionReceipt<S>, TxChangeSet), RejectReason>>,
}

impl<S: Spec> BackgroundTaskState<S> {
    fn shutdown(self) -> JoinHandle<BlockExecutionOutput<S>> {
        // Must be dropped before the result receiver, or a deadlock happens.
        drop(self.tx_sender);
        self.handle
    }
}

struct RollupBlockTaskContext<S: Spec> {
    checkpoint: StateCheckpoint<S>,
    old_rollup_height: RollupHeight,
    old_visible_slot_number: VisibleSlotNumber,
    next_visible_slot_number: VisibleSlotNumber,
    visible_increase: VisibleSlotNumberIncrease,
    // Channels
    // --------
    tx_receiver: mpsc::Receiver<FullyBakedTx>,
    setup_sender: oneshot::Sender<ChangeSet>,
    result_sender: mpsc::Sender<Result<(TransactionReceipt<S>, TxChangeSet), RejectReason>>,
    shutdown_notifier: mpsc::Sender<()>,
    // Config values
    // --------
    minimum_profit_per_tx: u128,
    admin_addresses: Arc<Vec<S::Address>>,
    sequencer_rollup_address: S::Address,
    sequencer_da_address: <S::Da as DaSpec>::Address,
}

fn rollup_block_task_body<S, Rt>(
    ctx: RollupBlockTaskContext<S>,
) -> (
    Vec<BatchReceipt<S>>,
    ChangeSet,
    StateAccesses,
    <S::Storage as Storage>::Witness,
)
where
    S: Spec,
    Rt: Runtime<S>,
{
    let RollupBlockTaskContext {
        mut checkpoint,
        old_rollup_height,
        old_visible_slot_number,
        next_visible_slot_number,
        visible_increase,
        tx_receiver,
        setup_sender,
        result_sender,
        shutdown_notifier,
        minimum_profit_per_tx,
        admin_addresses,
        sequencer_rollup_address,
        sequencer_da_address,
    } = ctx;

    let _span = tracing::trace_span!(
        "preferred_seq_bg_task",
        checkpoint_height = %checkpoint.rollup_height_to_access(),
    )
    .entered();

    let stf = StfBlueprint::<S, Rt>::new();
    let mut rt = Rt::default();
    let mut kernel = rt.kernel();
    let mut accessor: KernelStateAccessor<'_, S> =
        KernelStateAccessor::from_checkpoint(&kernel, &mut checkpoint);
    kernel.increment_rollup_height(&mut accessor, next_visible_slot_number);

    let next_root = kernel
        .visible_hash_for(old_rollup_height.saturating_add(1), &mut accessor)
        .ok_or_else(|| format!("Can't get visible hash for {old_rollup_height} + 1"))
        .unwrap();
    // Now that we've incremented the rollup height, we can get the next gas price. Do that and use it to compute the amount of funds that we should
    // reserve for the preferred sequencer.
    let next_gas_price = kernel
        .base_fee_per_gas(&mut accessor)
        .unwrap_or(S::initial_base_fee_per_gas());
    let needed_gas_escrow = S::max_tx_check_costs()
        .checked_value(&next_gas_price)
        .expect("Gas price overflow! This is a bug, please report it.");
    kernel.escrow_funds_for_preferred_sequencer(needed_gas_escrow, &mut accessor).expect("Failed to escrow funds for the preferred sequencer. The sequencer is too low on funds, which could cause soft confirmations to be invalidated. Increase your bond and restart the sequencer.");

    let blob_selector_output = {
        let preferred_blob = SelectedBlob {
            blob_data: BlobDataWithId::Batch(MaybeAsyncBatch::new_async(
                tx_receiver,
                setup_sender,
                result_sender,
                minimum_profit_per_tx,
                admin_addresses,
                sequencer_rollup_address,
            )),
            reserved_gas_tokens: Some(needed_gas_escrow),
            sender: sequencer_da_address.clone(),
        };

        let non_preferred_blobs = kernel
            .get_non_preferred_blobs(
                old_visible_slot_number
                    .as_true()
                    .next()
                    .range_inclusive(next_visible_slot_number.as_true()),
                &mut accessor,
                NoOpControlFlow,
            )
            .into_iter()
            .map(|mut b| {
                // Batches from unregistered sequencers don't reserve any gas
                // tokens.
                if b.reserved_gas_tokens.is_some() {
                    b.reserved_gas_tokens = Some(needed_gas_escrow);
                }
                b.map_batch(MaybeAsyncBatch::<S>::new_sync)
            })
            .collect::<Vec<_>>();

        tracing::debug!(count = %non_preferred_blobs.len(), "Extracted non-preferred blobs");

        let mut selected_blobs = vec![preferred_blob];
        selected_blobs.extend(non_preferred_blobs);

        BlobSelectorOutput {
            selected_blobs,
            visible_slot_number_increase: visible_increase.get().into(),
        }
    };

    tracing::trace!(
        %next_visible_slot_number,
        "Applying batches in user space"
    );
    let (_, _, batch_receipts, mut checkpoint) = stf.apply_batches_in_user_space(
        &mut Default::default(),
        blob_selector_output,
        checkpoint,
        ExecutionContext::Sequencer,
        next_root,
    );

    let mut changes = checkpoint.changes();
    let (accessory_delta, state_accesses, witness) =
        stf.materialize_accessory_state(&mut Default::default(), checkpoint);

    changes.changes.extend(
        accessory_delta
            .freeze()
            .into_iter()
            .map(|(k, v)| ((k.clone(), Namespace::Accessory), v.clone())),
    );
    drop(shutdown_notifier);

    (batch_receipts, changes, state_accesses, witness)
}

fn reject_reason_to_error(
    error: RejectReason,
    call_discriminant: impl std::fmt::Debug,
) -> ErrorObject {
    match error {
        RejectReason::SequencerOutOfGas => ErrorObject {
            status: StatusCode::SERVICE_UNAVAILABLE,
            title: "Batch is full".to_string(),
            details: json_obj!({
                "message": "More transactions were submitted that the sequencer is allowed to put into a single batch. Wait a few seconds and try again."
            }),
        },
        RejectReason::InsufficientReward { expected, found } => ErrorObject {
            status: StatusCode::FORBIDDEN,
            title: "Sequencer tip too low".to_string(),
            details: json_obj!({
                "message": "This transaction did not pay a sufficient net fee.",
                "minimum": expected,
                "found": found,
            }),
        },
        RejectReason::SenderMustBeAdmin => ErrorObject {
            status: StatusCode::FORBIDDEN,
            title: "The transaction is forbidden".to_string(),
            details: json_obj!({
                "message": format!("Only designated admins are allowed to send `{:#?}` transactions through this sequencer", call_discriminant),
            }),
        },
    }
}
