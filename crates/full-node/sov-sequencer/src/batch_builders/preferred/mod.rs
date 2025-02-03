//! See [`PreferredBatchBuilder`].

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
use sov_modules_api::capabilities::{
    BlobSelector, BlobSelectorOutput, ChainState, HasKernel, TransactionAuthenticator,
};
use sov_modules_api::rest::utils::{json_obj, ErrorObject};
use sov_modules_api::rest::ApiState;
use sov_modules_api::{
    BlobDataWithId, ChangeSet, ExecutionContext, FullyBakedTx, KernelStateAccessor,
    NestedEnumUtils, RawTx, RejectReason, Runtime, RuntimeEventProcessor, RuntimeEventResponse,
    Spec, StateCheckpoint, StateUpdateInfo, SyncStatus, TxChangeSet, VersionReader,
};
use sov_modules_stf_blueprint::{StfBlueprint, TransactionReceipt, TxEffect};
use sov_rest_utils::errors::database_error_500;
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::TxHash;
use sov_state::{Namespace, NativeStorage, StateUpdate, Storage};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{self, Sender};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, trace};

use super::{generic_accept_tx_error, RtAwareBatchBuilderSpec, SeqDbTx, SequencerConfirmation};
use crate::batch_builders::preferred::db::PreferredBbDbBlob;
use crate::batch_builders::{AcceptedTx, BatchBuilder, WithCachedTxHashes};
use crate::sequencer::SequencerNotReadyDetails;
use crate::{
    SequenceNumberProvider, Sequencer, SequencerConfig, SequencerSpec, TxStatus, TxStatusManager,
};

type VisibleSlotNumberIncrease = NonZero<u8>;

/// A batch builder with instant transaction confirmation.
pub struct PreferredBatchBuilder<Z: RtAwareBatchBuilderSpec> {
    db: PreferredBbDb<Z::Spec, Z::Rt>,
    state: InternalState<Z::Spec>,
    runtime: Z::Rt,
    checkpoint_sender: watch::Sender<StateCheckpoint<Z::Spec>>,
    api_state: ApiState<Z::Spec>,
    da_sync_state: Arc<DaSyncState>,
    latest_info: StateUpdateInfo<<Z::Spec as Spec>::Storage>,
    config: SequencerConfig<
        <Z::Spec as Spec>::Da,
        <Z::Spec as Spec>::Address,
        PreferredBatchBuilderConfig,
    >,
    next_event_number: u64,
    // A sender notifying that this acceptor has successfully shut down. We give a handle to
    // each background task when it is spawned, ensuring that this channel remains open as long
    // as any background task is operational even if the acceptor is dropped.
    shutdown_notifier: Sender<()>,
}

#[derive(derive_more::Debug)]
#[debug(bounds())]
enum InternalState<S: Spec> {
    /// Invalid state, used when we need to temporarily own the
    /// [`StateCheckpoint`].
    Placeholder,
    /// The [`BatchBuilder`] is currently idle and is not processing
    /// transactions for the next rollup block yet.
    Idle {
        checkpoint: StateCheckpoint<S>,
        /// When set to [`None`], the next rollup block is built on top of node
        /// state instead of sequencer state.
        ///
        /// See [`PreferredBatchBuilder::latest_info`].
        prev_state_root_opt: Option<<S::Storage as Storage>::Root>,
    },
    /// The [`BatchBuilder`] is currently accepting transactions from the
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
}

#[async_trait]
impl<Z: RtAwareBatchBuilderSpec> BatchBuilder for PreferredBatchBuilder<Z> {
    type Confirmation = Confirmation<Z>;
    type Batch = PreferredBatchData;
    type Config = PreferredBatchBuilderConfig;
    type Spec = Z::Spec;

    const PARALLEL_DA_SUBMISSION: bool = true;

    /// At the time of writing, the [`PreferredBatchBuilder`] doesn't use
    /// the [`TxStatusManager`].
    ///
    /// The [`Sequencer`] itself already updates the
    /// [`TxStatusManager`] after all operations, so we'd only need it if we
    /// ever "drop" previously-accepted transactions. The whole point of the
    /// [`PreferredBatchBuilder`] is that we *don't* do that.
    async fn create(
        latest_state_update: StateUpdateInfo<<Self::Spec as Spec>::Storage>,
        _tx_status_manager: TxStatusManager<<Self::Spec as Spec>::Da>,
        da_sync_state: Arc<DaSyncState>,
        storage_path: &Path,
        config: &SequencerConfig<<Z::Spec as Spec>::Da, <Z::Spec as Spec>::Address, Self::Config>,
    ) -> anyhow::Result<(Self, Option<JoinHandle<()>>)> {
        debug!(
            ?latest_state_update,
            "Instantiating the preferred batch builder"
        );

        let runtime: Z::Rt = Default::default();

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
        let shutdown_handle = tokio::task::spawn(async move {
            // This task blocks until we receive a notification that all
            // background tasks have been shut down.
            let _ = shutdown_rx.recv().await;
        });

        let mut bb = Self {
            db: PreferredBbDb::new(storage_path, &latest_state_update).await?,
            state: InternalState::node(&latest_state_update, &runtime),
            latest_info: latest_state_update.clone(),
            next_event_number: latest_state_update.next_event_number,
            api_state,
            checkpoint_sender,
            da_sync_state,
            shutdown_notifier,
            config: config.clone(),
            runtime,
        };

        // Restore soft-confirmed state that the node hasn't processed yet.
        bb.try_update_state(latest_state_update.clone()).await?;

        Ok((bb, Some(shutdown_handle)))
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
    async fn update_state(&mut self, info: StateUpdateInfo<<Z::Spec as Spec>::Storage>) {
        self.try_update_state(info).await.unwrap_or_else(|err| {
            error!(%err, "Failed to update preferred batch builder state. This failure is not recoverable, although application state is likely still intact and healthy. This is either a bug or a database issue.");
            std::process::exit(9); // Unique exit code so we can easily identify it from bug reports.
        });
    }

    fn encode_tx(raw: RawTx) -> FullyBakedTx {
        Z::Rt::encode_with_standard_auth(raw)
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn accept_tx(
        &mut self,
        baked_tx: FullyBakedTx,
    ) -> Result<AcceptedTx<Self::Confirmation>, ErrorObject> {
        if self
            .try_to_create_and_start_batch_if_none_in_progress(false)
            .await?
            .is_none()
        {
            panic!("No batch in progress, and no batch can be started. This is either because of (1) a bug, or (2) misuse of the `POST /sequencer/batches` endpoint. Please use automatic batch production exclusively, and report this bug if necessary. {:?} {:?}", self.state, self.latest_info);
        }

        let response = self.tx_confirmation(baked_tx.clone()).await;

        match &response {
            Ok(ok) => {
                trace!(
                    ?ok.confirmation.events,
                    "Transaction was accepted by the sequencer"
                );

                self.db
                    .insert_tx(&SeqDbTx::new(ok.tx_hash, baked_tx))
                    .await
                    .map_err(database_error_500)?;

                self.update_api_state().await;
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

    async fn assemble_batch(&mut self) -> anyhow::Result<Option<()>> {
        let checkpoint = match &self.state {
            InternalState::InProgressBatch { checkpoint, .. } |
            InternalState::Idle { checkpoint, .. } => checkpoint,
            InternalState::Placeholder => panic!("The sequencer contains an invalid internal state. This is a bug. Please report it."),
        };

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

        self.db.terminate_batch().await?;
        self.end_rollup_block_if_in_progress().await;

        self.update_api_state().await;
        Ok(Some(()))
    }

    async fn peek_batches(&mut self) -> anyhow::Result<Vec<WithCachedTxHashes<Self::Batch>>> {
        self.db.not_sent_yet_batches().await
    }

    async fn pop_batch(&mut self) -> anyhow::Result<()> {
        self.db.advance_not_sent_yet_cursor().await?;
        Ok(())
    }
}

impl<Z: RtAwareBatchBuilderSpec> PreferredBatchBuilder<Z> {
    /// The maximum number of transactions that can be buffered before incoming txs start getting
    /// rejected.
    const MAX_BUFFERED_TXS: usize = 1;

    /// Syncs [`ApiState`]s with the latest [`StateCheckpoint`].
    async fn update_api_state(&self) {
        let checkpoint = match &self.state {
            InternalState::Idle { checkpoint, .. }
            | InternalState::InProgressBatch { checkpoint, .. } => checkpoint,
            InternalState::Placeholder => panic!("The sequencer contains an invalid internal state. This is a bug, please report it."),
        };

        self.checkpoint_sender.send(
            checkpoint.clone_with_empty_witness(),
        ).expect("sending the checkpoint should never fail because one receiver is always present; this is a bug, please report it");
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
            .start_batch(visible_increase)
            .await
            .map_err(database_error_500)?;

        self.start_rollup_block(
            visible_increase,
            node_state_root,
            self.config.batch_builder.minimum_profit_per_tx,
        )
        .await;

        Ok(Some(()))
    }

    fn node_root_hash(&self) -> anyhow::Result<<<Z::Spec as Spec>::Storage as Storage>::Root> {
        self.latest_info
            .storage
            .get_root_hash(self.latest_info.slot_number)
    }

    #[tracing::instrument(skip_all)]
    async fn try_update_state(
        &mut self,
        info: StateUpdateInfo<<Z::Spec as Spec>::Storage>,
    ) -> anyhow::Result<()> {
        self.end_rollup_block_if_in_progress().await;

        self.next_event_number = info.next_event_number;
        self.latest_info = info;
        self.state = InternalState::node(&self.latest_info, &self.runtime);

        let batches_to_process = batches_to_process::<Z>(&self.db, &self.latest_info).await?;

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
            self.replay_batch(&batch).await?;
        }

        self.update_api_state().await;
        debug!("Sequencer state re-sync completed");

        Ok(())
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
                panic!(
                    "Transaction was soft-confirmed but failed to be re-applied; this is a bug, please report it {:?}",
                    err
                );
            }
        }

        trace!("Done replaying txs");
    }

    async fn start_rollup_block(
        &mut self,
        visible_increase: VisibleSlotNumberIncrease,
        // We pass the node state root explicitly because retrieving it is
        // fallible, so it's convenient to front-load the error-checking.
        node_state_root: <<Z::Spec as Spec>::Storage as Storage>::Root,
        minimum_profit_per_tx: u64,
    ) {
        self.start_rollup_block_inner(visible_increase, node_state_root, minimum_profit_per_tx)
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
        node_state_root: <<Z::Spec as Spec>::Storage as Storage>::Root,
        minimum_profit_per_tx: u64,
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

        let next_visible_slot_number = checkpoint
            .visible_slot_number_to_access()
            .advance(visible_increase.get().into());

        let prev_state_root = prev_state_root_opt.unwrap_or(node_state_root);

        let (setup_sender, setup_receiver) = oneshot::channel();
        let (tx_sender, tx_receiver) = mpsc::channel(Self::MAX_BUFFERED_TXS);
        let (result_sender, result_receiver) = mpsc::channel(Self::MAX_BUFFERED_TXS);

        let handle = tokio::runtime::Handle::current().spawn_blocking({
            let sequencer_address = self.config.da_address.clone();
            let admin_addresses = Arc::new(self.config.admin_addresses.clone());
            let shutdown_notifier = self.shutdown_notifier.clone();
            let additional_blobs = vec![]; // TODO.
            let mut checkpoint = checkpoint.clone_with_empty_witness();

            move || {
                let _span = tracing::trace_span!(
                    "preferred_bb_bg_task",
                    checkpoint_height = %checkpoint.rollup_height_to_access(),
                )
                .entered();

                let mut selected_blobs = vec![(
                    BlobDataWithId::Batch(AsyncBatch::new_async(
                        tx_receiver,
                        setup_sender,
                        result_sender,
                        minimum_profit_per_tx,
                        admin_addresses,
                    )),
                    sequencer_address,
                )];
                selected_blobs.extend(additional_blobs);
                let blob_selector_output = BlobSelectorOutput {
                    selected_blobs,
                    create_rollup_block: true,
                };
                let stf = StfBlueprint::<Z::Spec, Z::Rt>::new();
                let rt = Z::Rt::default();
                let kernel = rt.kernel();
                let mut accessor: KernelStateAccessor<'_, Z::Spec> =
                    KernelStateAccessor::from_checkpoint(&kernel, &mut checkpoint);
                kernel.increment_rollup_height(&mut accessor, next_visible_slot_number);
                tracing::info!(
                    %next_visible_slot_number,
                    "Applying batches in user space"
                );
                let (_, _, _, checkpoint) = stf.apply_batches_in_user_space(
                    blob_selector_output,
                    checkpoint,
                    ExecutionContext::Sequencer,
                    prev_state_root,
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

    /// Calls to this method must happen "between"
    /// [`PreferredBatchBuilder::start_rollup_block`] and
    /// [`PreferredBatchBuilder::end_rollup_block_if_in_progress`].
    #[tracing::instrument(skip_all, level = "trace")]
    async fn tx_confirmation(
        &mut self,
        baked_tx: FullyBakedTx,
    ) -> Result<AcceptedTx<Confirmation<Z>>, ErrorObject> {
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
        checkpoint: &mut StateCheckpoint<Z::Spec>,
        task_state: &mut BackgroundTaskState<Z::Spec>,
        next_event_number: u64,
    ) -> Result<AcceptedTx<Confirmation<Z>>, ErrorObject> {
        assert!(matches!(self.state, InternalState::Placeholder));

        let call = match Z::Rt::decode_serialized_tx(&self.runtime, &baked_tx) {
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
        checkpoint.apply_tx_changes(change_set);

        Ok(AcceptedTx {
            tx: baked_tx,
            tx_hash: receipt.tx_hash,
            confirmation: confirmation(receipt, next_event_number).unwrap(),
        })
    }
}

struct PreferredBatchToRestore {
    is_in_progress: bool,
    batch: WithCachedTxHashes<PreferredBatchData>,
}

/// Configuration for [`PreferredBatchBuilder`].
#[derive(
    Debug, Default, Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, JsonSchema,
)]
pub struct PreferredBatchBuilderConfig {
    /// The minimum fee that the preferred sequencer is willing to accept, denominated in rollup tokens. Defaults to zero.
    /// Sequencers should set this to a non-zero value if they wish to cover their DA costs.
    #[serde(default)]
    pub minimum_profit_per_tx: u64,
}

#[async_trait]
impl<Z, Ss> SequenceNumberProvider for Sequencer<Ss>
where
    Z: RtAwareBatchBuilderSpec,
    Ss: SequencerSpec<BatchBuilder = PreferredBatchBuilder<Z>>,
    //                ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    // One should not be able to use a non-preferred sequencer to produce
    // sequence numbers.
{
    async fn generate_sequence_number(&self, preferred_blob: &[u8]) -> anyhow::Result<u64> {
        self.batch_builder()
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

/// Transaction confirmation data of [`PreferredBatchBuilder`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Confirmation<Z: RtAwareBatchBuilderSpec> {
    tx_hash: TxHash,
    tx: Option<TxBody>,
    events: Vec<RuntimeEventResponse<<Z::Rt as RuntimeEventProcessor>::RuntimeEvent>>,
    receipt: TxEffect<Z::Spec>,
}

impl<Z: RtAwareBatchBuilderSpec> SequencerConfirmation for Confirmation<Z> {
    type EventInner = <Z::Rt as RuntimeEventProcessor>::RuntimeEvent;

    fn events(&self) -> Vec<RuntimeEventResponse<Self::EventInner>> {
        self.events.clone()
    }
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

fn confirmation<Z: RtAwareBatchBuilderSpec>(
    receipt: TransactionReceipt<Z::Spec>,
    next_event_number: u64,
) -> anyhow::Result<Confirmation<Z>> {
    Ok(Confirmation {
        tx_hash: receipt.tx_hash,
        tx: receipt.body_to_save.map(TxBody),
        events: receipt
            .events
            .into_iter()
            .zip(next_event_number..)
            .map(|(event, number)| {
                <RuntimeEventResponse<<Z::Rt as RuntimeEventProcessor>::RuntimeEvent>>::try_from((
                    number, event,
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
        receipt: receipt.receipt,
    })
}

async fn batches_to_process<Z: RtAwareBatchBuilderSpec>(
    db: &PreferredBbDb<Z::Spec, Z::Rt>,
    info: &StateUpdateInfo<<Z::Spec as Spec>::Storage>,
) -> anyhow::Result<Vec<PreferredBatchToRestore>> {
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
                    inner: batch.inner,
                    tx_hashes: batch.tx_hashes,
                },
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
            batch,
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
        .checked_sub(checkpoint.visible_slot_number_to_access().get());

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
