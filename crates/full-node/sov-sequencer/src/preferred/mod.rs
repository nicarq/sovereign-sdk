//! See [`PreferredSequencer`].

mod async_batch;
mod db;

use std::mem::replace;
use std::num::NonZero;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use async_batch::AsyncBatch;
use async_trait::async_trait;
use axum::http::StatusCode;
use db::PreferredBbDb;
use schemars::JsonSchema;
use serde_with::serde_as;
use sov_blob_storage::PreferredBatchData;
use sov_db::ledger_db::LedgerDb;
use sov_modules_api::capabilities::{BlobSelector, BlobSelectorOutput, ChainState};
use sov_modules_api::rest::utils::{json_obj, ErrorObject};
use sov_modules_api::rest::{ApiState, StateUpdateReceiver};
use sov_modules_api::{
    BlobDataWithId, ChangeSet, ExecutionContext, FullyBakedTx, Gas, GasSpec, KernelStateAccessor,
    NestedEnumUtils, RejectReason, Runtime, RuntimeEventProcessor, RuntimeEventResponse,
    SelectedBlob, Spec, StateCheckpoint, StateUpdateInfo, SyncStatus, TxChangeSet, VersionReader,
    VisibleSlotNumber,
};
use sov_modules_stf_blueprint::{StfBlueprint, TransactionReceipt, TxEffect};
use sov_rest_utils::errors::database_error_500;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::TxHash;
use sov_state::{Namespace, NativeStorage, StateRoot, StateUpdate, Storage};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{self, Sender};
use tokio::sync::{broadcast, oneshot, watch, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, error, trace};

use crate::blob_sender::BlobSender;
use crate::common::{
    generic_accept_tx_error, loop_call_update_state, loop_send_tx_notifications, AcceptedTx,
    SeqDbTx, Sequencer, WithCachedTxHashes,
};
use crate::preferred::db::PreferredBbDbBlob;
use crate::{
    SequenceNumberProvider, SequencerConfig, SequencerEvent, SequencerNotReadyDetails,
    SubmitBatchReceipt, TxStatus, TxStatusManager,
};

type VisibleSlotNumberIncrease = NonZero<u8>;

/// A inner batch builder struct containing state that requires synchronized access.
struct Inner<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    state: InternalState<S>,
    latest_info: StateUpdateInfo<S::Storage>,
    checkpoint_sender: watch::Sender<StateCheckpoint<S>>,
    next_event_number: u64,
    blob_sender: BlobSender<Da, PreferredBatchData>,
    db: PreferredBbDb<S, Rt>,
    runtime: Rt,
    config: SequencerConfig<S::Da, S::Address, PreferredSequencerConfig>,
    // A sender notifying that this acceptor has successfully shut down. We give a handle to
    // each background task when it is spawned, ensuring that this channel remains open as long
    // as any background task is operational even if the acceptor is dropped.
    shutdown_notifier: Sender<()>,
}

impl<S, Rt, Da> Inner<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    /// The maximum number of transactions that can be buffered before incoming txs start getting
    /// rejected.
    const MAX_BUFFERED_TXS: usize = 1;

    /// Syncs [`ApiState`]s with the latest [`StateCheckpoint`].
    async fn update_api_state(&self) {
        self.checkpoint_sender.send(
            self.state.checkpoint_ref().clone_with_empty_witness()
        ).expect("sending the checkpoint should never fail because one receiver is always present; this is a bug, please report it");
    }

    #[tracing::instrument(skip_all, fields(starting_from=%info.slot_number))]
    async fn update_state(&mut self, info: StateUpdateInfo<S::Storage>) -> anyhow::Result<()> {
        self.end_rollup_block_if_in_progress().await;

        self.next_event_number = info.next_event_number;
        self.latest_info = info;
        self.state = InternalState::node(&self.latest_info, &self.runtime);

        let batches_to_process = batches_to_process::<S, Rt>(&self.db, &self.latest_info).await?;

        {
            debug!(
                ?self.latest_info,
                "The sequencer will now re-apply transaction state changes on top of the latest state processed by the node"
            );

            let batch_details_to_log = batches_to_process
                .iter()
                .map(|batch| {
                    (
                        batch.batch.inner.sequence_number,
                        batch.batch.inner.visible_slots_to_advance,
                        batch.batch.inner.data.len(),
                    )
                })
                .collect::<Vec<_>>();
            trace!(
                ?batch_details_to_log,
                "Prepared batches to apply to the state"
            );
        }

        for batch in batches_to_process {
            if let Err(e) = self.replay_batch(&batch).await {
                tracing::error!("Error replaying batch: {:?}", e);
                std::process::exit(1);
            }
        }

        self.update_api_state().await;
        debug!("Sequencer state re-sync completed");

        if self.config.automatic_batch_production {
            if let Some(batch) = self.produce_batch_if_possible().await? {
                self.blob_sender.publish_batch(batch).await?;
            }
        }

        Ok(())
    }

    async fn produce_batch_if_possible(
        &mut self,
    ) -> anyhow::Result<Option<WithCachedTxHashes<PreferredBatchData>>> {
        let checkpoint = self.state.checkpoint_ref();

        // Check if we have enough slots to create a new batch immediately after
        // this one. If we don't, let's not assemble a batch.
        //
        // TODO(@neysofu): this check is currently necessary but likely can be folded into
        // `try_to_create_and_start_batch_if_none_in_progress`... somehow. As of
        // right now, it's a hair too bug-prone.
        if next_visible_slot_number_increase(checkpoint, &self.latest_info, true).is_none() {
            return Ok(None);
        }

        let new_batch_res = self.try_to_create_and_start_batch_if_none_in_progress(true)
            .await
            .map_err(|_| anyhow::anyhow!("Unable to start a new batch; this is likely a database issue or a bug, please report it"));

        if new_batch_res?.is_none() {
            return Ok(None);
        }

        let batch = self.db.terminate_batch().await?.batch;
        self.end_rollup_block_if_in_progress().await;

        self.update_api_state().await;
        Ok(Some(batch))
    }

    async fn end_rollup_block_if_in_progress(&mut self) {
        self.end_rollup_block_if_in_progress_inner().await;

        // Just a sanity check.
        assert!(
            matches!(self.state, InternalState::Idle { .. }),
            "Just ended a rollup block, but the state is not as expected ({:?}). This is a bug, please report it.",
            self.state
        );
    }

    async fn end_rollup_block_if_in_progress_inner(&mut self) {
        trace!("Ending rollup block");

        let (mut checkpoint, task_state) =
            match replace(&mut self.state, InternalState::Placeholder) {
                InternalState::InProgressBatch {
                    checkpoint,
                    task_state,
                } => (checkpoint, task_state),
                other => {
                    // Restore previous state.
                    self.state = other;

                    trace!("No in-progress rollup block, nothing to do");
                    return;
                }
            };

        let BackgroundTaskState {
            handle,
            tx_sender,
            result_receiver: _result_receiver,
        } = task_state;

        // Must be dropped before the result receiver, or a deadlock happens.
        drop(tx_sender);

        let (state_root, changes) = handle.await.expect(
            "Transaction acceptor task failed unexpectedly! This is a bug, please report it.",
        );

        checkpoint.apply_changes(changes);

        self.state = InternalState::Idle {
            checkpoint,
            prev_state_root_opt: Some(state_root),
        };

        trace!("Successfully ended rollup block");
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn replay_batch(&mut self, batch: &PreferredBatchToRestore) -> anyhow::Result<()> {
        assert!(
            matches!(self.state, InternalState::Idle { .. }),
            "Replaying a preferred batch, but the state is invalid and doesn't allow it ({:?}). This is a bug, please report it.",
            self.state
        );

        trace!(
            num_txs = batch.batch.inner.data.len(),
            "Re-applying batch state changes"
        );

        let node_state_root = self.node_root_hash()?;

        self.start_rollup_block(
            batch.batch.inner.visible_slots_to_advance,
            Some(batch.visible_slot_number_after_increase),
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
        self.replay_txs_in_preferred_batch(&batch.batch).await;

        if !batch.is_in_progress {
            self.end_rollup_block_if_in_progress().await;
        } else {
            trace!("The batch is still in progress; will keep the background task running");
        }

        Ok(())
    }

    async fn start_rollup_block(
        &mut self,
        visible_increase: VisibleSlotNumberIncrease,
        visible_slot_number_after_increase: Option<VisibleSlotNumber>,
        // We pass the node state root explicitly because retrieving it is
        // fallible, so it's convenient to front-load the error-checking.
        node_state_root: <S::Storage as Storage>::Root,
        minimum_profit_per_tx: u128,
    ) {
        self.start_rollup_block_inner(
            visible_increase,
            visible_slot_number_after_increase,
            node_state_root,
            minimum_profit_per_tx,
        )
        .await;

        // Just a sanity check.
        assert!(
            matches!(self.state, InternalState::InProgressBatch { .. }),
            "We just started a rollup block, but the state is not as expected ({:?}). This is a bug, please report it",
            self.state,
        );
    }

    async fn start_rollup_block_inner(
        &mut self,
        visible_increase: VisibleSlotNumberIncrease,
        visible_slot_number_after_increase: Option<VisibleSlotNumber>,
        node_state_root: <S::Storage as Storage>::Root,
        minimum_profit_per_tx: u128,
    ) {
        let InternalState::Idle {
            mut checkpoint,
            prev_state_root_opt,
        } = replace(&mut self.state, InternalState::Placeholder)
        else {
            panic!(
                "Unexpected sequencer state ({:?}), can't begin a new rollup block. This is a bug, please report it.",
                self.state
            );
        };

        trace!(
            ?checkpoint,
            ?self.latest_info,
            %visible_increase,
            "Beginning new rollup block and spawning background loop"
        );

        let computed_visible_slot_number = checkpoint
            .current_visible_slot_number()
            .advance(visible_increase.get().into());
        let next_visible_slot_number =
            visible_slot_number_after_increase.unwrap_or(computed_visible_slot_number);

        if let Some(visible_slot_number_after_increase) = visible_slot_number_after_increase {
            // TODO: Change this to an error log and a panic once all visible slot numbers fixes are merged
            tracing::debug!(
                "Overriding visible slot number from {} to: {}",
                computed_visible_slot_number,
                visible_slot_number_after_increase
            );
        }

        let prev_state_root = prev_state_root_opt.unwrap_or(node_state_root);

        let (setup_sender, setup_receiver) = oneshot::channel();
        let (tx_sender, tx_receiver) = mpsc::channel(Self::MAX_BUFFERED_TXS);
        let (result_sender, result_receiver) = mpsc::channel(Self::MAX_BUFFERED_TXS);

        let handle = tokio::runtime::Handle::current().spawn_blocking({
            let sequencer_address = self.config.da_address.clone();
            let sequencer_rollup_address = self.config.rollup_address.clone();
            let admin_addresses = Arc::new(self.config.admin_addresses.clone());
            let shutdown_notifier = self.shutdown_notifier.clone();
            let additional_blobs = vec![]; // TODO.
            let mut checkpoint = checkpoint.clone_with_empty_witness();
            let old_rollup_height = checkpoint.rollup_height_to_access();

            move || {
                let _span = tracing::trace_span!(
                    "preferred_seq_bg_task",
                    checkpoint_height = %checkpoint.rollup_height_to_access(),
                )
                .entered();

                let mut selected_blobs = vec![SelectedBlob {
                    blob_data: BlobDataWithId::Batch(AsyncBatch::new_async(
                        tx_receiver,
                        sequencer_rollup_address,
                        setup_sender,
                        result_sender,
                        minimum_profit_per_tx,
                        admin_addresses,
                    )),
                    reserved_gas_tokens: None, // We overwrite this value below.
                    sender: sequencer_address,
                }];
                selected_blobs.extend(additional_blobs);
                let mut blob_selector_output = BlobSelectorOutput {
                    selected_blobs,
                    visible_slot_number_increase: visible_increase.get().into(),
                };
                let stf = StfBlueprint::<S, Rt>::new();
                let rt = Rt::default();
                let kernel = rt.kernel();
                let mut accessor: KernelStateAccessor<'_, S> =
                    KernelStateAccessor::from_checkpoint(&kernel, &mut checkpoint);
                kernel.increment_rollup_height(
                    &mut accessor,
                    next_visible_slot_number,
                    &prev_state_root.namespace_root(sov_state::ProvableNamespace::User),
                );
                let next_root = kernel
                    .visible_hash_for(old_rollup_height.saturating_add(1), &mut accessor)
                    .unwrap();

                // Now that we've incremented the rollup height, we can get the next gas price. Do that and use it to compute the amount of funds that we should
                // reserve for the preferred sequencer.
                let next_gas_price = kernel
                    .base_fee_per_gas(&mut accessor)
                    .unwrap_or(S::initial_base_fee_per_gas());
                let needed_gas_escrow = S::max_tx_check_costs().checked_value(&next_gas_price).expect("Gas price overflow! This is a bug, please report it.");
                kernel.escrow_funds_for_preferred_sequencer(needed_gas_escrow, &mut accessor).expect("Failed to escrow funds for the preferred sequencer. The sequencer is too low on funds, which could cause soft confirmations to be invalidated. Increase your bond and restart the sequencer.");
                blob_selector_output.selected_blobs[0].reserved_gas_tokens = Some(needed_gas_escrow);



                tracing::info!(
                    %next_visible_slot_number,
                    "Applying batches in user space"
                );
                let (_, _, _, checkpoint) = stf.apply_batches_in_user_space(
                    blob_selector_output,
                    checkpoint,
                    ExecutionContext::Sequencer,
                    next_root,
                );
                let mut changes = checkpoint.changes();
                let (state_root, _witness, _change_set, state_update) =
                    stf.materialize_slot(true, checkpoint);
                changes.changes.extend(
                    state_update
                        .get_accessory_items()
                        .map(|(k, v)| ((k.clone(), Namespace::Accessory), v.clone())),
                );
                drop(shutdown_notifier);
                (state_root, changes)
            }
        });

        {
            // Wait for the background task to get up and running, and send the
            // initial change set.
            trace!("Applying setup changes...");
            let setup_changes = setup_receiver
                .await
                .with_context(|| "Setup must finish successfully")
                .expect("The batch builder can't recover from this error; this is a bug, please report it");
            trace!("Applied setup changes");

            checkpoint.apply_changes(setup_changes);
            checkpoint.advance_visible_slot_number(visible_increase);
        }

        self.state = InternalState::InProgressBatch {
            checkpoint,
            task_state: BackgroundTaskState {
                handle,
                tx_sender,
                result_receiver,
            },
        };
    }

    fn node_root_hash(&self) -> anyhow::Result<<S::Storage as Storage>::Root> {
        self.latest_info
            .storage
            .get_root_hash(self.latest_info.slot_number)
    }

    async fn replay_txs_in_preferred_batch(
        &mut self,
        batch: &WithCachedTxHashes<PreferredBatchData>,
    ) {
        trace!("Replaying txs");

        for (tx, tx_hash) in batch.inner.data.iter().zip(batch.tx_hashes.iter()) {
            trace!(
                %tx_hash,
                "Re-applying state changes for the soft-confirmed transaction"
            );

            if let Err(err) = self.tx_confirmation(tx.clone()).await {
                tracing::error!(
                    "Transaction was soft-confirmed but failed to be re-applied; this is a bug, please report it {:?}",
                    err
                );
                std::process::exit(1);
            }
        }

        trace!("Done replaying txs");
    }

    /// Calls to this method must happen "between"
    /// [`PreferredSequencer::start_rollup_block`] and
    /// [`PreferredSequencer::end_rollup_block_if_in_progress`].
    #[tracing::instrument(skip_all, level = "trace")]
    async fn tx_confirmation(
        &mut self,
        baked_tx: FullyBakedTx,
    ) -> Result<AcceptedTx<Confirmation<S, Rt>>, ErrorObject> {
        let InternalState::InProgressBatch {
            mut checkpoint,
            mut task_state,
        } = replace(&mut self.state, InternalState::Placeholder)
        else {
            panic!("Accepting a transaction, yet there's no in-progress batch ({:?}). This is a bug in the sequencer, please report it.", self.state);
        };

        let response = self
            .tx_confirmation_inner_result(
                baked_tx,
                &mut checkpoint,
                &mut task_state,
                self.next_event_number,
            )
            .await;

        self.state = InternalState::InProgressBatch {
            checkpoint,
            task_state,
        };

        if let Ok(ref ok) = response {
            self.next_event_number += ok.confirmation.events.len() as u64;
        }

        response
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn tx_confirmation_inner_result(
        &self,
        baked_tx: FullyBakedTx,
        checkpoint: &mut StateCheckpoint<S>,
        task_state: &mut BackgroundTaskState<S>,
        next_event_number: u64,
    ) -> Result<AcceptedTx<Confirmation<S, Rt>>, ErrorObject> {
        assert!(matches!(self.state, InternalState::Placeholder));

        let call = match Rt::decode_serialized_tx(&self.runtime, &baked_tx) {
            Ok((call, _)) => call,
            Err(e) => {
                let error = ErrorObject {
                    status: StatusCode::BAD_REQUEST,
                    title: "Malformed transaction".to_string(),
                    details: json_obj!({
                        "message": format!("This transaction could not be deserialized. {e}",)
                    }),
                };

                return Err(error);
            }
        };

        // Send the transaction for execution
        if let Err(TrySendError::Full(_)) = task_state.tx_sender.try_send(baked_tx.clone()) {
            let error = ErrorObject {
                status: StatusCode::SERVICE_UNAVAILABLE, // 503
                title: "Temporarily unavailable".to_string(),
                details: json_obj!({
                    "message": "The sequencer is temporarily overloaded. Try again in a few seconds."
                }),
            };
            return Err(error);
        }
        let result = task_state
            .result_receiver
            .recv()
            .await
            .expect("The background task failed unexpectedly");

        let (receipt, change_set) = match result {
            Ok(receipt) => receipt,
            Err(reason) => return Err(reject_reason_to_error(reason, call.discriminant())),
        };

        if !receipt.receipt.is_successful() {
            return Err(generic_accept_tx_error(receipt.receipt));
        }

        // If we made it this far, the tx was successful. Update our state with the changes and accept.
        checkpoint.apply_changes(change_set.0);

        Ok(AcceptedTx {
            tx: baked_tx,
            tx_hash: receipt.tx_hash,
            confirmation: confirmation(receipt, next_event_number).unwrap(),
        })
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn try_to_create_and_start_batch_if_none_in_progress(
        &mut self,
        leave_space_for_next_batch: bool,
    ) -> Result<Option<()>, ErrorObject> {
        let checkpoint = match &self.state {
            InternalState::Idle { checkpoint, .. } => checkpoint,
            InternalState::InProgressBatch { .. } => return Ok(Some(())),
            InternalState::Placeholder => panic!("The sequencer contains an invalid internal state. This is a bug, please report it."),
        };

        let Some(visible_increase) = next_visible_slot_number_increase(
            checkpoint,
            &self.latest_info,
            leave_space_for_next_batch,
        ) else {
            return Ok(None);
        };

        debug!(visible_increase, "No in-progress batch, starting a new one");

        let node_state_root = self.node_root_hash().map_err(database_error_500)?;

        // If the database operation fails here it's okay because we still
        // haven't touched the background task nor modified `self`, so
        // everything will be left in a valid state.
        self.db
            .start_batch(
                VisibleSlotNumber::new_dangerous(
                    self.latest_info.latest_finalized_slot_number.get(),
                ),
                visible_increase,
            )
            .await
            .map_err(database_error_500)?;

        self.start_rollup_block(
            visible_increase,
            None,
            node_state_root,
            self.config.sequencer_kind_config.minimum_profit_per_tx,
        )
        .await;

        Ok(Some(()))
    }
}

/// A [`Sequencer`] with instant transaction confirmation.
pub struct PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    inner: Mutex<Inner<S, Rt, Da>>,
    tx_status_manager: TxStatusManager<S::Da>,
    events_sender: broadcast::Sender<SequencerEvent<Rt>>,
    api_state: ApiState<S>,
    da_sync_state: Arc<DaSyncState>,
}

#[derive(derive_more::Debug)]
#[debug(bounds())]
enum InternalState<S: Spec> {
    /// Invalid state, used when we need to temporarily own the
    /// [`StateCheckpoint`].
    Placeholder,
    /// The [`Sequencer`] is currently idle and is not processing
    /// transactions for the next rollup block yet.
    Idle {
        checkpoint: StateCheckpoint<S>,
        /// When set to [`None`], the next rollup block is built on top of node
        /// state instead of sequencer state.
        ///
        /// See [`PreferredSequencer::latest_info`].
        prev_state_root_opt: Option<<S::Storage as Storage>::Root>,
    },
    /// The [`Sequencer`] is currently accepting transactions from the
    /// preferred batch of a rollup block. Note that every rollup block
    /// (under normal operations, not e.g. in recovery mode) has exactly one
    /// preferred batch.
    InProgressBatch {
        checkpoint: StateCheckpoint<S>,
        task_state: BackgroundTaskState<S>,
    },
}

impl<S: Spec> InternalState<S> {
    fn node(info: &StateUpdateInfo<S::Storage>, runtime: &impl Runtime<S>) -> Self {
        let checkpoint = StateCheckpoint::new(info.storage.clone(), &runtime.kernel());

        InternalState::Idle {
            checkpoint,
            prev_state_root_opt: None,
        }
    }

    pub fn checkpoint_ref(&self) -> &StateCheckpoint<S> {
        match self {
            InternalState::Idle { checkpoint, .. }
            | InternalState::InProgressBatch { checkpoint, .. } => checkpoint,
            InternalState::Placeholder => panic!("The sequencer contains an invalid internal state. This is a bug, please report it."),
        }
    }
}

#[async_trait]
impl<S, Rt, Da> Sequencer for PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    type Confirmation = Confirmation<S, Rt>;
    type Config = PreferredSequencerConfig;
    type Spec = S;
    type Rt = Rt;
    type Da = Da;

    /// At the time of writing, the [`PreferredSequencer`] doesn't use
    /// the [`TxStatusManager`].
    ///
    /// The [`Sequencer`] itself already updates the
    /// [`TxStatusManager`] after all operations, so we'd only need it if we
    /// ever "drop" previously-accepted transactions. The whole point of the
    /// [`PreferredSequencer`] is that we *don't* do that.
    async fn create(
        da: Da,
        state_update_receiver: StateUpdateReceiver<S::Storage>,
        da_sync_state: Arc<DaSyncState>,
        storage_path: &Path,
        config: &SequencerConfig<S::Da, S::Address, Self::Config>,
        ledger_db: LedgerDb,
        shutdown_receiver: watch::Receiver<()>,
    ) -> anyhow::Result<(Arc<Self>, Vec<JoinHandle<()>>)> {
        let latest_state_update = state_update_receiver.borrow().clone();
        debug!(
            ?latest_state_update,
            "Instantiating the preferred batch builder"
        );

        let runtime: Rt = Default::default();
        let tx_status_manager = TxStatusManager::default();

        assert!(
            accepts_preferred_batches(runtime.blob_selector()),
            "Attempting to use preferred sequencer with an incompatible rollup. Set your sequencer config to `standard` in your rollup's config.toml file or change your kernel to be compatible with soft confirmations."
        );

        let (checkpoint_sender, checkpoint_receiver) = watch::channel(StateCheckpoint::new(
            latest_state_update.storage.clone(),
            &runtime.kernel(),
        ));
        let api_state = ApiState::build(
            Arc::new(()),
            checkpoint_receiver,
            runtime.kernel_with_slot_mapping(),
            None,
        );

        let (shutdown_notifier, mut shutdown_rx) = mpsc::channel(1);
        let mut handles = vec![tokio::task::spawn(async move {
            // This task blocks until we receive a notification that all
            // background tasks have been shut down.
            let _ = shutdown_rx.recv().await;
        })];

        let (events_sender, _) =
            broadcast::channel(config.sequencer_kind_config.events_channel_size);

        let mut inner = Inner {
            db: PreferredBbDb::new(storage_path, &latest_state_update).await?,
            blob_sender: BlobSender::new(
                da,
                storage_path,
                tx_status_manager.clone(),
                true,
                shutdown_receiver.clone(),
            )
            .await?,
            state: InternalState::node(&latest_state_update, &runtime),
            latest_info: latest_state_update.clone(),
            checkpoint_sender,
            next_event_number: latest_state_update.next_event_number,
            config: config.clone(),
            runtime,
            shutdown_notifier: shutdown_notifier.clone(),
        };

        inner.update_state(latest_state_update.clone()).await?;

        let seq = Arc::new(PreferredSequencer {
            inner: inner.into(),
            tx_status_manager,
            events_sender,
            da_sync_state,
            api_state,
        });

        handles.push(tokio::spawn({
            loop_call_update_state(
                seq.clone(),
                state_update_receiver.clone(),
                shutdown_receiver.clone(),
            )
        }));
        handles.push(tokio::spawn({
            let ledger_db = ledger_db.clone();
            let seq = seq.clone();
            async move {
                loop_send_tx_notifications::<S, Rt>(
                    state_update_receiver,
                    shutdown_receiver,
                    &ledger_db,
                    seq.tx_status_manager(),
                )
                .await;
            }
        }));

        Ok((seq, handles))
    }

    fn is_ready(&self) -> Result<(), SequencerNotReadyDetails> {
        let status = self.da_sync_state.status();

        match status {
            SyncStatus::Synced { .. } => Ok(()),
            SyncStatus::Syncing {
                synced_da_height,
                target_da_height,
            } => {
                let distance = status.distance();
                if distance <= sov_blob_storage::config_deferred_slots_count() {
                    Ok(())
                } else {
                    Err(SequencerNotReadyDetails {
                        target_da_height,
                        synced_da_height,
                    })
                }
            }
        }
    }

    fn api_state(&self) -> ApiState<Self::Spec> {
        self.api_state.clone()
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn update_state(&self, info: StateUpdateInfo<S::Storage>) {
        let mut inner = self.inner.lock().await;
        inner.update_state(info).await.unwrap_or_else(|err| {
            error!(%err, "Failed to update preferred batch builder state. This failure is not recoverable, although application state is likely still intact and healthy. This is either a bug or a database issue.");
            std::process::exit(9); // Unique exit code so we can easily identify it from bug reports.
        });
    }

    fn tx_status_manager(&self) -> &TxStatusManager<<Self::Spec as Spec>::Da> {
        &self.tx_status_manager
    }

    async fn subscribe_events(&self) -> Option<broadcast::Receiver<SequencerEvent<Rt>>> {
        Some(self.events_sender.subscribe())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn accept_tx(
        &self,
        baked_tx: FullyBakedTx,
    ) -> Result<AcceptedTx<Self::Confirmation>, ErrorObject> {
        let mut inner = self.inner.lock().await;
        if inner
            .try_to_create_and_start_batch_if_none_in_progress(false)
            .await?
            .is_none()
        {
            panic!("No batch in progress, and no batch can be started. This is either because of (1) a bug, or (2) misuse of the `POST /sequencer/batches` endpoint. Please use automatic batch production exclusively, and report this bug if necessary. {:?} {:?}", inner.state, inner.latest_info);
        }

        let response = inner.tx_confirmation(baked_tx.clone()).await;

        match &response {
            Ok(ok) => {
                trace!(
                    ?ok.confirmation.events,
                    "Transaction was accepted by the sequencer"
                );

                inner
                    .db
                    .insert_tx(&SeqDbTx::new(ok.tx_hash, baked_tx))
                    .await
                    .map_err(database_error_500)?;

                inner.update_api_state().await;

                self.tx_status_manager
                    .notify(ok.tx_hash, TxStatus::Submitted);

                for event in ok.confirmation.events.iter().cloned() {
                    self.events_sender
                        .send(SequencerEvent {
                            tx_hash: ok.tx_hash,
                            event,
                        })
                        .ok();
                }
            }
            Err(error) => {
                debug!(error.title, "Transaction was rejected by the sequencer");
            }
        }

        response
    }

    async fn tx_status(
        &self,
        _tx_hash: &TxHash,
    ) -> anyhow::Result<
        TxStatus<<<Self::Spec as Spec>::Da as sov_modules_api::DaSpec>::TransactionId>,
    > {
        // At the time of writing, information in the DB is not stored in such a
        // way that facilitates random access to tx status information. That
        // means the sequencer only relies on the cache. FIXME(@neysofu).
        Ok(TxStatus::Unknown)
    }

    async fn submit_batch(
        &self,
        txs: Vec<FullyBakedTx>,
    ) -> anyhow::Result<Option<SubmitBatchReceipt<Da::Spec>>> {
        for tx in txs.iter() {
            self.accept_tx(tx.clone()).await.ok(); // FIXME(@neysofu): handle error.
        }

        let mut inner = self.inner.lock().await;

        if let Some(batch) = inner.produce_batch_if_possible().await? {
            inner
                .blob_sender
                .publish_batch_and_wait(batch)
                .await
                .map(Some)
        } else {
            Ok(None)
        }
    }
}

struct PreferredBatchToRestore {
    is_in_progress: bool,
    visible_slot_number_after_increase: VisibleSlotNumber,
    batch: WithCachedTxHashes<PreferredBatchData>,
}

/// Configuration for [`PreferredSequencer`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, JsonSchema)]
pub struct PreferredSequencerConfig {
    /// The minimum fee that the preferred sequencer is willing to accept, denominated in rollup tokens. Defaults to zero.
    /// Sequencers should set this to a non-zero value if they wish to cover their DA costs.
    #[serde(default)]
    pub minimum_profit_per_tx: u128,
    /// The size of the Tokio channel used to stream events.
    ///
    /// Don't deviate from the default unless you know what you're doing.
    #[serde(default = "default_events_channel_size")]
    pub events_channel_size: usize,
}

impl Default for PreferredSequencerConfig {
    fn default() -> Self {
        Self {
            minimum_profit_per_tx: 0,
            events_channel_size: default_events_channel_size(),
        }
    }
}

fn default_events_channel_size() -> usize {
    100
}

#[async_trait]
impl<S, Rt, Da> SequenceNumberProvider for PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    async fn generate_sequence_number(&self, preferred_blob: &[u8]) -> anyhow::Result<u64> {
        self.inner
            .lock()
            .await
            .db
            .insert_proof_blob(preferred_blob.to_vec())
            .await
    }
}

#[derive(Debug)]
struct BackgroundTaskState<S: Spec> {
    handle: JoinHandle<(<<S as Spec>::Storage as Storage>::Root, ChangeSet)>,
    tx_sender: mpsc::Sender<FullyBakedTx>,
    result_receiver: mpsc::Receiver<Result<(TransactionReceipt<S>, TxChangeSet), RejectReason>>,
}

#[serde_with::serde_as]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TxBody(#[serde_as(as = "serde_with::base64::Base64")] Vec<u8>);

/// Transaction confirmation data of [`PreferredSequencer`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(bound = "S: Spec, Rt: Runtime<S>")]
pub struct Confirmation<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    tx_hash: TxHash,
    tx: Option<TxBody>,
    events: Vec<RuntimeEventResponse<<Rt as RuntimeEventProcessor>::RuntimeEvent>>,
    receipt: TxEffect<S>,
}

fn reject_reason_to_error(
    error: RejectReason,
    call_discriminant: impl std::fmt::Debug,
) -> ErrorObject {
    match error {
        // TODO: get appropriate number of slots to advance.
        // TODO: There's a complicated edge case here where the sequencer doesn't have enough stake for the number of incoming transactions
        // (recall that the sequencer must have enough take to cover all N authentication attempts in order to submit a batch of size N).
        // In that case, this check will fail repeatedly in a short time window. However, the sequencer is only allowed to submit 1 batch
        // per slot. In that case, the "correct" solution is to raise the required fees per transaction and plow the profits into increasing
        // the sequencer's stake.
        // Finally, there's one small edge case where the sequencer isn't staked enough to cover even a single tx. In that case, we should
        // probably throw an error on startup.
        RejectReason::SequencerOutOfGas => {
            todo!("The sequencer ran out of gas! Support for recovery is not yet implemented");
            #[allow(unreachable_code)]
            ErrorObject {
                status: StatusCode::SERVICE_UNAVAILABLE,
                title: "Batch is full".to_string(),
                details: json_obj!({
                    "message": "More transactions were submitted that the sequencer is allowed to put into a single batch. Wait a few seconds and try again."
                }),
            }
        }
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

fn confirmation<S, Rt>(
    receipt: TransactionReceipt<S>,
    next_event_number: u64,
) -> anyhow::Result<Confirmation<S, Rt>>
where
    S: Spec,
    Rt: Runtime<S>,
{
    Ok(Confirmation {
        tx_hash: receipt.tx_hash,
        tx: receipt.body_to_save.map(TxBody),
        events: receipt
            .events
            .into_iter()
            .zip(next_event_number..)
            .map(|(event, number)| {
                <RuntimeEventResponse<<Rt as RuntimeEventProcessor>::RuntimeEvent>>::try_from((
                    number, event,
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
        receipt: receipt.receipt,
    })
}

async fn batches_to_process<S, Rt>(
    db: &PreferredBbDb<S, Rt>,
    info: &StateUpdateInfo<S::Storage>,
) -> anyhow::Result<Vec<PreferredBatchToRestore>>
where
    S: Spec,
    Rt: Runtime<S>,
{
    let blobs_to_apply = match db.all_subsequent_blobs(info).await {
        Ok(b) => b,
        Err(err) => {
            error!(%err, "Database error while re-applying state changes. This is a critical error. Database integrity is intact, but the sequencer may momentarily provide outdated state and break soft-confirmations.");
            return Err(err);
        }
    };

    let first_sequence_number = blobs_to_apply.first().map(|b| b.sequence_number());

    trace!(
        blobs_count = blobs_to_apply.len(),
        first_sequence_number,
        last_sequence_number = blobs_to_apply.last().map(|b| b.sequence_number()),
        "Extracted blobs to apply from database"
    );

    let mut batches: Vec<_> = blobs_to_apply
        .into_iter()
        .filter_map(|blob| match blob {
            PreferredBbDbBlob::Batch(batch) => Some(PreferredBatchToRestore {
                is_in_progress: false,
                batch: WithCachedTxHashes {
                    inner: batch.batch.inner,
                    tx_hashes: batch.batch.tx_hashes,
                },
                visible_slot_number_after_increase: batch.visible_slot_number_after_increase,
            }),
            // TODO(https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/2063): Process proofs.
            // Note: once we start processing proofs in addition to batches,
            // we gotta make sure to order everything by sequence number as
            // proofs can have a sequence number that's greater than the
            // in-progress batch.
            _ => {
                trace!(
                    sequence_number = %blob.sequence_number(),
                    "Ignoring proof blob"
                );
                None
            }
        })
        .collect();

    if let Some(batch) = db.in_progress_batch_opt().await? {
        batches.push(PreferredBatchToRestore {
            is_in_progress: true,
            visible_slot_number_after_increase: batch.visible_slot_number_after_increase,
            batch: batch.batch,
        });
    }

    Ok(batches)
}

fn next_visible_slot_number_increase<S: Spec>(
    checkpoint: &StateCheckpoint<S>,
    info: &StateUpdateInfo<S::Storage>,
    leave_space_for_next_batch: bool,
) -> Option<NonZero<u8>> {
    trace!(?checkpoint, ?info, %leave_space_for_next_batch, "Calculating next visible slot number");

    let mut delta = info
        .latest_finalized_slot_number
        .checked_sub(checkpoint.current_visible_slot_number().get());

    if leave_space_for_next_batch {
        delta = delta.and_then(|x| x.checked_sub(1));
    }

    match delta {
        Some(delta) => NonZero::new(delta.get().try_into().unwrap_or(u8::MAX)),
        None => None,
    }
}

/// A helper function to allow recovering an associated consant from an *instance* of a type
/// when the type itself is unknown. This is useful when a function returns `impl Trait` and we
/// want to get an associated item from that trait implementation.
fn accepts_preferred_batches<B: BlobSelector>(_blob_selector: B) -> bool {
    B::ACCEPTS_PREFERRED_BATCHES
}
