//! See [`PreferredBatchBuilder`].

mod db;

use std::num::NonZero;
use std::path::Path;
use std::sync::Arc;

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
    BlobDataWithId, DaSpec, ExecutionContext, FullyBakedTx, KernelStateAccessor, NestedEnumUtils,
    RawTx, RejectReason, RuntimeEventProcessor, RuntimeEventResponse, Spec, StateCheckpoint,
    StateUpdateInfo, SyncStatus, TxChangeSet, VersionReader,
};
use sov_modules_stf_blueprint::{StfBlueprint, TransactionReceipt, TxEffect};
use sov_rest_utils::errors::database_error_500;
use sov_rollup_interface::common::VisibleSlotNumber;
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::TxHash;
use sov_state::{NativeStorage, Storage};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, error, trace};

use super::{generic_accept_tx_error, RtAwareBatchBuilderSpec, SeqDbTx, SequencerConfirmation};
use crate::batch_builders::preferred::db::PreferredBbDbBlob;
use crate::batch_builders::{AcceptedTx, BatchBuilder, WithCachedTxHashes};
use crate::sequencer::SequencerNotReadyDetails;
use crate::{
    SequenceNumberProvider, Sequencer, SequencerConfig, SequencerSpec, TxStatus, TxStatusManager,
};

mod async_batch;

type AsyncBlobAndSender<Z> = (
    BlobDataWithId<
        AsyncBatch<
            TransactionReceipt<<Z as RtAwareBatchBuilderSpec>::Spec>,
            <Z as RtAwareBatchBuilderSpec>::Spec,
        >,
    >,
    <<<Z as RtAwareBatchBuilderSpec>::Spec as Spec>::Da as DaSpec>::Address,
);

type TxResult<Z> = Result<
    (
        TransactionReceipt<<Z as RtAwareBatchBuilderSpec>::Spec>,
        TxChangeSet,
    ),
    RejectReason,
>;

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

/// A batch builder with instant transaction confirmation.
pub struct PreferredBatchBuilder<Z: RtAwareBatchBuilderSpec> {
    db: PreferredBbDb<Z::Spec, Z::Rt>,
    checkpoint: Option<StateCheckpoint<Z::Spec>>,
    checkpoint_sender: watch::Sender<StateCheckpoint<Z::Spec>>,
    api_state: ApiState<Z::Spec>,
    da_sync_state: Arc<DaSyncState>,
    next_event_number: u64,
    acceptor: TxAcceptor<Z>,
}

/// A helper function to allow recovering an associated consant from an *instance* of a type
/// when the type itself is unknown. This is useful when a function returns `impl Trait` and we
/// want to get an associated item from that trait implementation.
fn accepts_preferred_batches<B: BlobSelector>(_blob_selector: B) -> bool {
    B::ACCEPTS_PREFERRED_BATCHES
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
        let runtime: Z::Rt = Default::default();
        let blob_selector = runtime.blob_selector();
        assert!(accepts_preferred_batches(blob_selector), "Attempting to use preferred sequencer with an incompatible rollup. Set your sequencer config to `standard` in your rollup's config.toml file or change your kernel to be compatible with soft confirmations.");
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

        let db = PreferredBbDb::new(storage_path, &latest_state_update).await?;

        let initial_checkpoint =
            StateCheckpoint::new(latest_state_update.storage.clone(), &runtime.kernel());

        // TODO: Use an older state root if necessary. cc @neysofu
        let initial_height = latest_state_update.slot_number;
        let initial_state_root = latest_state_update
            .storage
            .get_root_hash(initial_height)
            .expect("Latest rollup height must be present in database");

        let (result_sender, result_receiver) =
            tokio::sync::mpsc::channel(TxAcceptor::<Z>::MAX_BUFFERED_TXS);

        debug!(
            %initial_height,
            %latest_state_update.latest_finalized_slot_number,
            ?initial_state_root,
            "Instantiating the preferred batch builder"
        );

        let latest_finalized_slot_number = latest_state_update.latest_finalized_slot_number;

        let blobs_to_restore = match db.all_subsequent_blobs(&latest_state_update).await {
            Ok(b) => b,
            Err(err) => {
                error!(%err, "Database error while re-applying state changes. This is a critical error. Database integrity is intact, but the sequencer may momentarily provide outdated state and break soft-confirmations.");
                return Err(err);
            }
        };
        let next_visible_slot_number = {
            let mut current_visible_slot_number =
                initial_checkpoint.visible_slot_number_to_access();
            if let Some(next_height_increase) = blobs_to_restore
                .iter()
                .filter_map(|blob| blob.visible_slots_to_advance())
                .next()
            {
                current_visible_slot_number.advance(next_height_increase as u64)
            } else {
                // Update the visible slot number to the latest finalized slot number if possible. However,
                // we're only allowed to update it by at most u8::MAX slots at a time.
                std::cmp::min(
                    latest_finalized_slot_number.as_visible(),
                    current_visible_slot_number.advance(u8::MAX as u64),
                )
            }
        };

        let (acceptor, shutdown_handle) = TxAcceptor::new(
            initial_checkpoint.clone_with_empty_witness(),
            initial_state_root,
            next_visible_slot_number,
            vec![], // TODO(https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/2063): provide any missing blobs from the DA layer / DB
            result_sender,
            result_receiver,
            config.clone(),
        );
        let mut bb = Self {
            db,
            acceptor,
            next_event_number: latest_state_update.next_event_number,
            checkpoint: Some(initial_checkpoint),
            checkpoint_sender,
            api_state,
            da_sync_state,
        };

        // Restore soft-confirmed state that the node hasn't processed yet.
        bb.try_update_state_with_blobs(latest_state_update, blobs_to_restore)
            .await?;

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

    async fn update_state(&mut self, info: StateUpdateInfo<<Z::Spec as Spec>::Storage>) {
        self.try_update_state(info).await.unwrap_or_else(|err| {
            error!(%err, "Failed to update preferred batch builder state. This failure is not recoverable, although application state is likely still intact and healthy. This is either a bug or a database issue.");
            std::process::exit(9); // Unique exit code so we can easily identify it from bug reports.
        });
    }

    fn encode_tx(raw: RawTx) -> FullyBakedTx {
        Z::Rt::encode_with_standard_auth(raw)
    }

    async fn accept_tx(
        &mut self,
        baked_tx: FullyBakedTx,
    ) -> Result<AcceptedTx<Self::Confirmation>, ErrorObject> {
        self.start_batch_if_needed().await?;

        let old_checkpoint = self
            .checkpoint
            .take()
            .expect("Absent checkpoint; this is a bug, please report it");

        let (new_checkpoint, response) = self
            .acceptor
            .tx_confirmation(baked_tx.clone(), old_checkpoint, self.next_event_number)
            .await;
        self.checkpoint = Some(new_checkpoint);

        match &response {
            Ok(ok) => {
                trace!(
                    ?ok.confirmation.events,
                    "Transaction was accepted by the sequencer"
                );

                self.next_event_number += ok.confirmation.events.len() as u64;

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

    async fn assemble_batch(&mut self) -> anyhow::Result<()> {
        self.start_batch_if_needed().await.map_err(|_| anyhow::anyhow!("Unable to start a new batch; this is likely a database issue or a bug, please report it"))?;
        self.db.terminate_batch().await?;

        let checkpoint = self
            .checkpoint
            .as_mut()
            .expect("Missing internal checkpoint; this is a bug, please report it");

        self.acceptor
            .move_to_next_slot(
                checkpoint.clone_with_empty_witness(),
                // TODO(https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/2063): provide proofs and non-preferred blobs here.
                vec![],
                checkpoint.visible_slot_number_to_access().advance(1), // TODO: This breaks if we call submit batch more frequently than blocks finalize.
                None,
            )
            .await;

        Ok(())
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
    /// Syncs [`ApiState`]s with the latest [`StateCheckpoint`].
    async fn update_api_state(&self) {
        let checkpoint = self
            .checkpoint
            .as_ref()
            .expect("Missing internal checkpoint; this is a bug, please report it")
            .clone_with_empty_witness();

        self.checkpoint_sender.send(
            checkpoint
        ).expect("sending the checkpoint should never fail because one receiver is always present; this is a bug, please report it");
    }

    async fn start_batch_if_needed(&mut self) -> Result<(), ErrorObject> {
        if self.db.sequence_number_of_in_progress_batch.is_none() {
            let next_visible_slot_number_increase = self.next_visible_slot_number_increase();

            debug!(
                next_visible_slot_number_increase,
                "No in-progress batch, starting a new one"
            );

            self.db
                .start_batch(next_visible_slot_number_increase)
                .await
                .map_err(database_error_500)?;
        }

        Ok(())
    }

    async fn try_update_state(
        &mut self,
        info: StateUpdateInfo<<Z::Spec as Spec>::Storage>,
    ) -> anyhow::Result<()> {
        let blobs_to_restore = match self.db.all_subsequent_blobs(&info).await {
            Ok(b) => b,
            Err(err) => {
                error!(%err, "Database error while re-applying state changes. This is a critical error. Database integrity is intact, but the sequencer may momentarily provide outdated state and break soft-confirmations.");
                return Err(err);
            }
        };

        self.try_update_state_with_blobs(info, blobs_to_restore)
            .await
    }

    #[tracing::instrument(skip_all)]
    async fn try_update_state_with_blobs(
        &mut self,
        info: StateUpdateInfo<<Z::Spec as Spec>::Storage>,
        blobs_to_apply: Vec<PreferredBbDbBlob>,
    ) -> anyhow::Result<()> {
        debug!(
            ?info,
            "The sequencer is now re-applying transaction state changes on top of the latest state processed by the node"
        );

        let mut checkpoint =
            StateCheckpoint::new(info.storage.clone(), &self.acceptor.runtime.kernel());

        trace!(
            checkpoint_height = %checkpoint.rollup_height_to_access(),
            "Re-applying state changes"
        );

        let batches_to_process = {
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
                    PreferredBbDbBlob::Batch(batch) => Some((
                        true,
                        WithCachedTxHashes {
                            inner: batch.inner,
                            tx_hashes: batch.tx_hashes,
                        },
                    )),
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

            if let Some(batch) = self.db.in_progress_batch_opt().await? {
                batches.push((false, batch));
            }

            batches
        };

        {
            let txs_count_by_sequence_number =
                batches_to_process.iter().map(|(is_complete, batch)| {
                    (
                        is_complete,
                        batch.inner.sequence_number,
                        batch.inner.data.len(),
                    )
                });
            trace!(
                txs_count_by_sequence_number = ?txs_count_by_sequence_number.collect::<Vec<_>>(),
                "Prepared batches to apply to the state"
            );
        }

        // Reset the acceptor state inside a new slot.

        let mut last_batch_was_complete = false;
        for (idx, (is_complete, batch)) in batches_to_process.iter().enumerate() {
            last_batch_was_complete = *is_complete;
            let next_visible_slot_number = {
                let mut current_visible_slot_number = checkpoint.visible_slot_number_to_access();
                current_visible_slot_number
                    .advance(batch.inner.visible_slots_to_advance.get() as u64)
            };
            trace!(
                idx,
                num_txs = batch.inner.data.len(),
                "Re-applying batch state changes"
            );
            let root = if idx == 0 {
                Some(info.storage.get_root_hash(info.slot_number)?)
            } else {
                None
            };

            self.acceptor
                .move_to_next_slot(
                    checkpoint.clone_with_empty_witness(),
                    vec![],
                    next_visible_slot_number,
                    root,
                )
                .await;

            checkpoint = self.replay_batch(batch, checkpoint).await;
        }

        if last_batch_was_complete {
            let next_visible_slot_number = {
                let mut current_visible_slot_number = checkpoint.visible_slot_number_to_access();
                // Update the visible slot number to the latest finalized slot number if possible. However,
                // we're only allowed to update it by at most u8::MAX slots at a time.
                std::cmp::min(
                    info.latest_finalized_slot_number.as_visible(),
                    current_visible_slot_number.advance(u8::MAX as u64),
                )
            };

            self.acceptor
                .move_to_next_slot(
                    checkpoint.clone_with_empty_witness(),
                    vec![],
                    next_visible_slot_number,
                    None,
                )
                .await;
        }

        self.checkpoint = Some(checkpoint);
        self.update_api_state().await;

        Ok(())
    }

    async fn replay_batch(
        &mut self,
        batch: &WithCachedTxHashes<PreferredBatchData>,
        mut checkpoint: StateCheckpoint<Z::Spec>,
    ) -> StateCheckpoint<Z::Spec> {
        for (tx, tx_hash) in batch.inner.data.iter().zip(batch.tx_hashes.iter()) {
            trace!(
                tx_hash = %tx_hash,
                "Re-applying state changes for the soft-confirmed transaction"
            );

            let (new_checkpoint, response) = self
                .acceptor
                .tx_confirmation(tx.clone(), checkpoint, self.next_event_number)
                .await;
            checkpoint = new_checkpoint;

            match response {
                Ok(ref ok) => {
                    self.next_event_number += ok.confirmation.events.len() as u64;
                }
                Err(err) => {
                    panic!(
                        "Transaction was soft-confirmed but failed to be re-applied; this is a bug, please report it {:?}",
                        err
                    );
                }
            }
        }

        checkpoint
    }

    fn next_visible_slot_number_increase(&self) -> NonZero<u8> {
        // TODO(@neysofu): finalized height -aware visible slot number increase logic.
        NonZero::new(1).unwrap()
    }
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

type AcceptTxResult<Z> = (
    StateCheckpoint<<Z as RtAwareBatchBuilderSpec>::Spec>,
    Result<AcceptedTx<Confirmation<Z>>, ErrorObject>,
);

/// Subset of the [`PreferredBatchBuilder`] state that is needed to accept a
/// transaction.
struct TxAcceptor<Z: RtAwareBatchBuilderSpec> {
    runtime: Z::Rt,
    tx_sender: Sender<FullyBakedTx>,
    result_receiver: Receiver<TxResult<Z>>,
    // Optional because we temporarily take the handle during loop re-initialization.
    // Callers may safely `unwrap` because we hold a `&mut self` any time the handle is `None`.`
    handle: Option<JoinHandle<<<Z::Spec as Spec>::Storage as Storage>::Root>>,
    // A sender notifying that this acceptor has successfully shut down. We give a handle to
    // each background task when it is spawned, ensuring that this channel remains open as long
    // as any background task is operational even if the acceptor is dropped.
    shutdown_notifier: Sender<()>,
    admin_addresses: Arc<Vec<<Z::Spec as Spec>::Address>>,
    sequencer_address: <<Z::Spec as Spec>::Da as DaSpec>::Address,
    minimum_profit_per_tx: u64,
}

impl<Z: RtAwareBatchBuilderSpec> TxAcceptor<Z> {
    /// The maximum number of transactions that can be buffered before incoming txs start getting
    /// rejected.
    pub const MAX_BUFFERED_TXS: usize = 1;

    pub fn new(
        checkpoint: StateCheckpoint<<Z as RtAwareBatchBuilderSpec>::Spec>,
        initial_state_root: <<Z::Spec as Spec>::Storage as Storage>::Root,
        next_visible_slot_number: VisibleSlotNumber,
        additional_blobs: Vec<AsyncBlobAndSender<Z>>,
        result_sender: Sender<TxResult<Z>>,
        result_receiver: Receiver<TxResult<Z>>,
        config: SequencerConfig<
            <Z::Spec as Spec>::Da,
            <Z::Spec as Spec>::Address,
            PreferredBatchBuilderConfig,
        >,
    ) -> (Self, JoinHandle<()>) {
        let (tx_sender, tx_receiver) = tokio::sync::mpsc::channel(Self::MAX_BUFFERED_TXS);
        let (shutdown_notifier, mut shutdown_rx) = tokio::sync::mpsc::channel(1);

        let handle = Some(Self::start_background_loop(
            checkpoint,
            tx_receiver,
            result_sender,
            additional_blobs,
            Arc::new(config.admin_addresses.clone()),
            config.da_address.clone(),
            initial_state_root,
            next_visible_slot_number,
            config.batch_builder.minimum_profit_per_tx,
            shutdown_notifier.clone(),
        ));

        let shutdown_handle = tokio::task::spawn(async move {
            // This task blocks until we receive a notification that all
            // background tasks have been shut down.
            let _ = shutdown_rx.recv().await;
        });

        (
            Self {
                runtime: Z::Rt::default(),
                tx_sender,
                result_receiver,
                handle,
                admin_addresses: Arc::new(config.admin_addresses),
                sequencer_address: config.da_address,
                minimum_profit_per_tx: config.batch_builder.minimum_profit_per_tx,
                shutdown_notifier,
            },
            shutdown_handle,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start_background_loop(
        mut checkpoint: StateCheckpoint<Z::Spec>,
        tx_receiver: Receiver<FullyBakedTx>,
        result_sender: Sender<TxResult<Z>>,
        additional_blobs: Vec<AsyncBlobAndSender<Z>>,
        admin_addresses: Arc<Vec<<Z::Spec as Spec>::Address>>,
        sequencer_address: <<Z::Spec as Spec>::Da as DaSpec>::Address,
        initial_state_root: <<Z::Spec as Spec>::Storage as Storage>::Root,
        next_visible_slot_number: VisibleSlotNumber,
        minimum_profit_per_tx: u64,
        shutdown_notifier: Sender<()>,
    ) -> JoinHandle<<<Z::Spec as Spec>::Storage as Storage>::Root> {
        trace!(
            height = %checkpoint.rollup_height_to_access(),
            "Spawning background loop"
        );

        tokio::runtime::Handle::current().spawn_blocking(move || {
            let _span = tracing::trace_span!(
                "sequencer_tx_acceptor",
                checkpoint_height = %checkpoint.rollup_height_to_access(),
            )
            .entered();

            let mut selected_blobs = vec![(
                BlobDataWithId::Batch(AsyncBatch::new_async(
                    tx_receiver,
                    result_sender.clone(),
                    minimum_profit_per_tx,
                    admin_addresses,
                )),
                sequencer_address,
            )];
            selected_blobs.extend(additional_blobs);
            let blob_selector_output = BlobSelectorOutput {
                selected_blobs,
                should_execute_slot_hooks: true,
            };
            let stf = StfBlueprint::<Z::Spec, Z::Rt>::new();
            let rt = Z::Rt::default();
            let kernel = rt.kernel();
            let mut accessor: KernelStateAccessor<'_, Z::Spec> =
                KernelStateAccessor::from_checkpoint(&kernel, &mut checkpoint);
            kernel.increment_rollup_height(&mut accessor, next_visible_slot_number);
            tracing::info!(
                "Applying batches in user space. using visible_slot_number: {}",
                next_visible_slot_number
            );
            let (_, _, _, checkpoint) = stf.apply_batches_in_user_space(
                blob_selector_output,
                checkpoint,
                ExecutionContext::Sequencer,
                initial_state_root,
            );
            let (state_root, _witness, _change_set) = stf.materialize_slot(true, checkpoint);
            drop(shutdown_notifier);
            state_root
        })
    }

    async fn finish_background_loop_iter(
        &mut self,
    ) -> Option<(
        <<Z::Spec as Spec>::Storage as Storage>::Root,
        Receiver<FullyBakedTx>,
        Sender<TxResult<Z>>,
    )> {
        let (tx_sender, tx_receiver) = tokio::sync::mpsc::channel(Self::MAX_BUFFERED_TXS);
        let (result_sender, result_receiver) = tokio::sync::mpsc::channel(Self::MAX_BUFFERED_TXS);
        // Drop the sender to the current background task. This causes it to finish its current batch rather than blocking until it receives more transactions.
        self.tx_sender = tx_sender;
        self.result_receiver = result_receiver;
        if let Some(handle) = std::mem::take(&mut self.handle) {
            Some( (handle
            .await
            .expect(
                "Transaction acceptor task failed unexpectedly! This is a bug, please report it.",
            ), tx_receiver, result_sender))
        } else {
            None
        }
    }

    /// This function is tightly coupled with the implementation of the
    /// background task. It works by...
    ///  1. Closing the existing tx channel. This causes the background task to
    ///     close out the current batch and begin applying any "forced" blobs
    ///     immediately, then awaiting the final result.
    ///  2. Starting a new background task.
    async fn move_to_next_slot(
        &mut self,
        new_checkpoint: StateCheckpoint<Z::Spec>,
        additional_blobs: Vec<AsyncBlobAndSender<Z>>,
        next_visible_slot_number: VisibleSlotNumber,
        state_root: Option<<<Z::Spec as Spec>::Storage as Storage>::Root>,
    ) {
        trace!(
            height = %new_checkpoint.rollup_height_to_access(),
            "Moving to next slot"
        );
        let (prev_state_root, tx_receiver, result_sender) = self
            .finish_background_loop_iter()
            .await
            .expect("Missing join handle in sequencer! This is a bug, please report it.");
        trace!(
            height = %new_checkpoint.rollup_height_to_access(),
            "Starting background loop"
        );
        // TODO: Apply remaining changes from batch to new checkpoint.
        self.handle = Some(Self::start_background_loop(
            new_checkpoint,
            tx_receiver,
            result_sender,
            additional_blobs,
            self.admin_addresses.clone(),
            self.sequencer_address.clone(),
            // We simply use the state root from the previous slot if no new
            // state root was provided. A user may provide a different state
            // root if they wish to process a slot that's not the next one, e.g.
            // when replaying transactions on top of old state.
            state_root.unwrap_or(prev_state_root),
            next_visible_slot_number,
            self.minimum_profit_per_tx,
            self.shutdown_notifier.clone(),
        ));
    }

    async fn tx_confirmation(
        &mut self,
        baked_tx: FullyBakedTx,
        mut checkpoint: StateCheckpoint<Z::Spec>,
        next_event_number: u64,
    ) -> AcceptTxResult<Z> {
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

                return (checkpoint, Err(error));
            }
        };

        // Send the transaction for execution
        if let Err(TrySendError::Full(_)) = self.tx_sender.try_send(baked_tx.clone()) {
            let error = ErrorObject {
                status: StatusCode::SERVICE_UNAVAILABLE, // 503
                title: "Temporarily unavailable".to_string(),
                details: json_obj!({
                    "message": "The sequencer is temporarily overloaded. Try again in a few seconds."
                }),
            };
            return (checkpoint, Err(error));
        }
        let result = self
            .result_receiver
            .recv()
            .await
            .expect("The background task failed unexpectedly");

        let (receipt, change_set) = match result {
            Ok(receipt) => receipt,
            Err(reason) => {
                return (
                    checkpoint,
                    Err(reject_reason_to_error(reason, call.discriminant())),
                )
            }
        };

        if !receipt.receipt.is_successful() {
            return (checkpoint, Err(generic_accept_tx_error(receipt.receipt)));
        }

        // If we made it this far, the tx was successful. Update our state with the changes and accept.
        checkpoint.apply_changes(change_set);

        let accepted_tx = AcceptedTx {
            tx: baked_tx,
            tx_hash: receipt.tx_hash,
            confirmation: confirmation(receipt, next_event_number).unwrap(),
        };

        (checkpoint, Ok(accepted_tx))
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
