//! See [`PreferredBatchBuilder`].

use std::sync::Arc;

use async_batch::AsyncBatch;
use async_trait::async_trait;
use axum::http::StatusCode;
use schemars::JsonSchema;
use serde_with::serde_as;
use sov_blob_storage::PreferredBatchData;
use sov_db::sequencer_db::SeqDbTx;
use sov_modules_api::capabilities::{BlobSelectorOutput, HasKernel, TransactionAuthenticator};
use sov_modules_api::rest::ApiState;
use sov_modules_api::{
    BlobDataWithId, DaSpec, ExecutionContext, FullyBakedTx, NestedEnumUtils, RawTx, RejectReason,
    RuntimeEventProcessor, RuntimeEventResponse, Spec, StateCheckpoint, StateUpdateInfo,
    SyncStatus, TxChangeSet,
};
use sov_modules_stf_blueprint::{StfBlueprint, TransactionReceipt, TxEffect};
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::TxHash;
use sov_state::{NativeStorage, Storage};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use super::{generic_accept_tx_error, RtAwareBatchBuilderSpec, SequencerConfirmation};
use crate::batch_builders::{
    AcceptTxError, AcceptedTx, BatchBuilder, FreshlyBuiltBatch, TxWithHash,
};
use crate::sequencer::SequencerNotReadyDetails;
use crate::{SeqDbTxExtend, SequencerConfig, TxStatusManager};

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
    checkpoint: Option<StateCheckpoint<<Z::Spec as Spec>::Storage>>,
    checkpoint_sender: watch::Sender<StateCheckpoint<<Z::Spec as Spec>::Storage>>,
    api_state: ApiState<Z::Spec>,
    da_sync_state: Arc<DaSyncState>,
    txs_in_next_batch: Vec<TxWithHash>,
    next_event_number: u64,
    acceptor: TxAcceptor<Z>,
    config: PreferredBatchBuilderConfig,
}

impl<Z: RtAwareBatchBuilderSpec> PreferredBatchBuilder<Z> {
    async fn reapply_txs(&mut self, txs: &[SeqDbTx]) {
        let mut checkpoint = self
            .checkpoint
            .take()
            .expect("Absent checkpoint; this is a bug, please report it");

        for seqdb_tx in txs {
            let tx_input = seqdb_tx.tx_input::<Self>();

            let (new_checkpoint, response) = self
                .acceptor
                .tx_confirmation(tx_input, checkpoint, self.next_event_number)
                .await;

            checkpoint = new_checkpoint;

            match response {
                Ok(ref ok) => {
                    self.txs_in_next_batch.push(TxWithHash {
                        fully_baked_tx: seqdb_tx.fully_baked_tx(),
                        hash: seqdb_tx.hash,
                    });
                    self.next_event_number += ok.confirmation.events.len() as u64;
                }
                Err(err) => {
                    warn!(
                        ?err,
                        "Failed to restore transaction; this is likely indicative of an abrupt sequencer shutdown. Please monitor logs and report any potential issues.",
                    );
                }
            }
        }

        self.checkpoint = Some(checkpoint);
        self.checkpoint_sender
            .send(
                self.checkpoint
                    .as_ref()
                    .expect("Missing internal checkpoint; this is a bug, please report it")
                    .clone_with_empty_witness(),
            )
            .ok();
    }
}

/// Configuration for [`PreferredBatchBuilder`].
#[derive(
    Debug, Default, Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, JsonSchema,
)]
pub struct PreferredBatchBuilderConfig {
    /// Whether the sequencer should update its state to track the received state of the full-node before submitting a batch.
    /// ## TODO(@theochap)
    /// This is a temporary solution to prevent breakage of the sequencer. It should be removed once we have fully integrated
    /// the sequencer and fixed update race conditions.
    #[serde(default)]
    pub should_update_state: bool,
    /// The minimum fee that the preferred sequencer is willing to accept, denominated in rollup tokens. Defaults to zero.
    /// Sequencers should set this to a non-zero value if they wish to cover their DA costs.
    #[serde(default)]
    pub minimum_profit_per_tx: u64,
}

#[async_trait]
impl<Z: RtAwareBatchBuilderSpec> BatchBuilder for PreferredBatchBuilder<Z> {
    type TxInput = <Z::Rt as TransactionAuthenticator<Z::Spec>>::Input;
    type Confirmation = Confirmation<Z>;
    type Batch = PreferredBatchData;
    type Config = PreferredBatchBuilderConfig;
    type Spec = Z::Spec;

    async fn create(
        latest_state_update: StateUpdateInfo<<Self::Spec as Spec>::Storage>,
        da_sync_state: Arc<DaSyncState>,
        seq_db_txs: Vec<SeqDbTx>,
        config: &SequencerConfig<<Z::Spec as Spec>::Da, <Z::Spec as Spec>::Address, Self::Config>,
    ) -> anyhow::Result<(Self, Option<JoinHandle<()>>)> {
        let runtime: Z::Rt = Default::default();
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

        let initial_checkpoint =
            StateCheckpoint::new(latest_state_update.storage.clone(), &runtime.kernel());

        let admin_addresses = Arc::new(config.admin_addresses.clone());

        // TODO: Use an older state root if necessary. cc @neysofu
        let initial_height = latest_state_update.rollup_height;
        let initial_state_root = latest_state_update
            .storage
            .get_root_hash(initial_height)
            .expect("Latest rollup height must be present in database");
        let (acceptor, shutdown_handle) = TxAcceptor::new(
            Default::default(),
            config.da_address.clone(),
            admin_addresses.clone(),
            vec![], // TODO: provide any missing blobs from the DA layer / DB
            initial_checkpoint.clone_with_empty_witness(),
            initial_state_root,
            config.batch_builder.minimum_profit_per_tx,
        );
        let mut bb = Self {
            acceptor,
            next_event_number: latest_state_update.next_event_number,
            checkpoint: Some(initial_checkpoint),
            checkpoint_sender,
            api_state,
            da_sync_state,
            txs_in_next_batch: vec![],
            config: config.batch_builder.clone(),
        };

        // Restore persisted transactions.
        bb.reapply_txs(&seq_db_txs).await;

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

    fn tx_status_manager(&self) -> TxStatusManager<<Z::Spec as Spec>::Da> {
        TxStatusManager::default()
    }

    async fn update_state(&mut self, info: StateUpdateInfo<<Z::Spec as Spec>::Storage>) {
        // TODO(@theochap): remove this once we have fully integrated the sequencer and fixed update race conditions.
        // If [`Inner::should_update_state`] is set, we update the state of the batch builder to the
        // one received from the full-node before submitting a batch.
        if self.config.should_update_state {
            let txs_to_process = self.txs_in_next_batch.clone();
            let checkpoint = StateCheckpoint::new(info.storage.clone(), &Z::Rt::default().kernel());

            self.checkpoint = Some(checkpoint);

            debug!(
                da_height = info.rollup_height,
                num_txs_to_process = txs_to_process.len(),
                "The sequencer is now re-applying transaction state changes on top of the latest state processed by the node"
            );

            for (idx, tx) in txs_to_process.iter().enumerate() {
                trace!(
                    idx,
                    tx_hash = %tx.hash,
                    "Re-applying state changes for the soft-confirmed transaction"
                );

                let tx_input = borsh::from_slice(&tx.fully_baked_tx.data)
                    .expect("Failed to deserialize transaction");
                if let Err(error) = self.accept_tx(tx_input).await {
                    warn!(
                        ?error,
                        "Transaction was soft-confirmed but failed to be re-applied"
                    );
                }
            }

            self.next_event_number = info.next_event_number;
            self.checkpoint_sender
                .send(self.checkpoint.as_ref().unwrap().clone_with_empty_witness())
                .ok();
        }
    }

    fn encode_tx(raw: RawTx) -> Self::TxInput {
        Z::Rt::add_standard_auth(raw)
    }

    async fn accept_tx(
        &mut self,
        tx_input: Self::TxInput,
    ) -> Result<AcceptedTx<Self::Confirmation>, AcceptTxError> {
        let old_checkpoint = self
            .checkpoint
            .take()
            .expect("Absent checkpoint; this is a bug, please report it");

        let (new_checkpoint, response) = self
            .acceptor
            .tx_confirmation(tx_input, old_checkpoint, self.next_event_number)
            .await;
        self.checkpoint = Some(new_checkpoint);

        if let Ok(ref ok) = response {
            self.next_event_number += ok.confirmation.events.len() as u64;

            self.txs_in_next_batch.push(TxWithHash {
                fully_baked_tx: ok.tx.clone(),
                hash: ok.tx_hash,
            });

            self.checkpoint_sender
                .send(
                    self.checkpoint
                        .as_ref()
                        .expect("Missing internal checkpoint; this is a bug, please report it")
                        .clone_with_empty_witness(),
                )
                .ok();
        }

        response
    }

    async fn build_next_batch(
        &mut self,
        sequence_number: u64,
    ) -> anyhow::Result<FreshlyBuiltBatch<Self>> {
        // TODO: Compute the correct set of blobs to use here.
        self.acceptor
            .move_to_next_slot(
                self.checkpoint
                    .as_ref()
                    .expect("Missing internal checkpoint; this is a bug, please report it")
                    .clone_with_empty_witness(),
                vec![],
            )
            .await;
        let (txs, hashes) = self
            .txs_in_next_batch
            .iter()
            .map(|tx| (tx.fully_baked_tx.clone(), tx.hash))
            .unzip();

        let batch = FreshlyBuiltBatch {
            inner: PreferredBatchData {
                data: txs,
                virtual_slots_to_advance: 1,
                sequence_number,
            },
            hashes,
        };

        Ok(batch)
    }

    async fn clear_batch(&mut self) -> anyhow::Result<()> {
        self.txs_in_next_batch.clear();
        let checkpoint = self
            .checkpoint
            .as_ref()
            .expect("Missing internal checkpoint; this is a bug, please report it")
            .clone_with_empty_witness();
        // We have to shut down the current tx_acceptor
        // TODO: Compute the correct list of blobs to add to this batch
        self.acceptor.move_to_next_slot(checkpoint, vec![]).await;
        Ok(())
    }
}

type AcceptTxResult<Z> = (
    StateCheckpoint<<<Z as RtAwareBatchBuilderSpec>::Spec as Spec>::Storage>,
    Result<AcceptedTx<Confirmation<Z>>, AcceptTxError>,
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
        runtime: Z::Rt,
        sequencer_address: <<Z::Spec as Spec>::Da as DaSpec>::Address,
        admin_addresses: Arc<Vec<<Z::Spec as Spec>::Address>>,
        additional_blobs: Vec<AsyncBlobAndSender<Z>>,
        checkpoint: StateCheckpoint<<<Z as RtAwareBatchBuilderSpec>::Spec as Spec>::Storage>,
        initial_state_root: <<Z::Spec as Spec>::Storage as Storage>::Root,
        minimum_profit_per_tx: u64,
    ) -> (Self, JoinHandle<()>) {
        let (tx_sender, tx_receiver) = tokio::sync::mpsc::channel(Self::MAX_BUFFERED_TXS);
        let (result_sender, result_receiver) = tokio::sync::mpsc::channel(Self::MAX_BUFFERED_TXS);
        let (shutdown_notifier, mut shutdown_rx) = tokio::sync::mpsc::channel(1);

        let handle = Some(Self::start_background_loop(
            checkpoint,
            tx_receiver,
            result_sender,
            additional_blobs,
            admin_addresses.clone(),
            sequencer_address.clone(),
            initial_state_root,
            minimum_profit_per_tx,
            shutdown_notifier.clone(),
        ));

        let shutdown_handle = tokio::task::spawn(async move {
            // This task blocks until we receive a notification that all background tasks have been shut down
            let _ = shutdown_rx.recv().await;
        });

        (
            Self {
                runtime,
                tx_sender,
                result_receiver,
                handle,
                admin_addresses,
                sequencer_address,
                minimum_profit_per_tx,
                shutdown_notifier,
            },
            shutdown_handle,
        )
    }

    fn start_background_loop(
        checkpoint: StateCheckpoint<<Z::Spec as Spec>::Storage>,
        tx_receiver: Receiver<FullyBakedTx>,
        result_sender: Sender<TxResult<Z>>,
        additional_blobs: Vec<AsyncBlobAndSender<Z>>,
        admin_addresses: Arc<Vec<<Z::Spec as Spec>::Address>>,
        sequencer_address: <<Z::Spec as Spec>::Da as DaSpec>::Address,
        initial_state_root: <<Z::Spec as Spec>::Storage as Storage>::Root,
        minimum_profit_per_tx: u64,
        shutdown_notifier: Sender<()>,
    ) -> JoinHandle<<<Z::Spec as Spec>::Storage as Storage>::Root> {
        tokio::runtime::Handle::current().spawn_blocking(move || {
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

    /// This function is tightly coupled with the implementation of the background task. It works by...
    /// 1. Closing the existing tx channel. This causes the background task to close out the current batch and begin applying any
    /// "forced" blobs immediately, then awaiting the final result
    /// 2. Starting a new background task
    async fn move_to_next_slot(
        &mut self,
        new_checkpoint: StateCheckpoint<<Z::Spec as Spec>::Storage>,
        additional_blobs: Vec<AsyncBlobAndSender<Z>>,
    ) {
        let (prev_state_root, tx_receiver, result_sender) = self
            .finish_background_loop_iter()
            .await
            .expect("Missing join handle in sequencer! This is a bug, please report it.");
        // TODO: Apply remaining changes from batch to new checkpoint.
        self.handle = Some(Self::start_background_loop(
            new_checkpoint,
            tx_receiver,
            result_sender,
            additional_blobs,
            self.admin_addresses.clone(),
            self.sequencer_address.clone(),
            prev_state_root,
            self.minimum_profit_per_tx,
            self.shutdown_notifier.clone(),
        ));
    }

    async fn tx_confirmation(
        &mut self,
        tx_input: <Z::Rt as TransactionAuthenticator<Z::Spec>>::Input,
        mut checkpoint: StateCheckpoint<<Z::Spec as Spec>::Storage>,
        next_event_number: u64,
    ) -> AcceptTxResult<Z> {
        let call = match Z::Rt::parse_input(&self.runtime, &tx_input) {
            Ok((call, _)) => call,
            Err(e) => {
                return (
                    checkpoint,
                    Err(AcceptTxError {
                        http_status: StatusCode::BAD_REQUEST.as_u16(),
                        title: "Malformed transaction".to_string(),
                        details: format!("This transaction could not be deserialized. {e}",),
                    }),
                );
            }
        };
        let baked_tx = Z::Rt::encode_athenticator_input(&tx_input);

        // Send the transaction for execution
        if let Err(TrySendError::Full(_)) = self.tx_sender.try_send(baked_tx.clone()) {
            let error = AcceptTxError {
                http_status: StatusCode::SERVICE_UNAVAILABLE.as_u16(), // 503
                title: "Temporarily unavailable".to_string(),
                details: "The sequencer is temporarily overloaded. Try again in a few seconds."
                    .to_string(),
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
            Err(e) => match e {
                // TODO: get appropriate number of slots to advance.
                // TODO: There's a complicated edge case here where the sequencer doesn't have enough stake for the number of incoming transactions
                // (recall that the sequencer must have enough take to cover all N authentication attempts in order to submit a batch of size N).
                // In that case, this check will fail repeatedly in a short time window. However, the sequencer is only allowed to submit 1 batch
                // per slot. In that case, the "correct" solution is to raise the required fees per transaction and plow the profits into increasing
                // the sequencer's stake. 
                // Finally, there's one small edge case where the sequencer isn't staked enough to cover even a single tx. In that case, we should
                // probably throw an error on startup.
                RejectReason::SequencerOutOfGas =>  {
                    todo!("The sequencer ran out of gas! Support for recovery is not yet implemented");
                    #[allow(unreachable_code)]
                    return (checkpoint, Err(AcceptTxError {
                        http_status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                        title: "Batch is full".to_string(),
                        details: "More transactions were submitted that the sequencer is allowed to put into a single batch. Wait a few seconds and try again.".to_string(),
                    }))
                  },
                RejectReason::InsufficientReward { expected, found } => return (checkpoint, Err(AcceptTxError {
                    http_status: StatusCode::FORBIDDEN.as_u16(),
                    title: "Sequencer tip too low".to_string(),
                    details: format!(
                        "This transaction did not pay a sufficient net fee. Minimum: {expected}. Found: {found}"
                    ),
                })),
                RejectReason::SenderMustBeAdmin => {
                    return (checkpoint, Err(AcceptTxError {
                        http_status: StatusCode::FORBIDDEN.as_u16(),
                        title: "The transaction is forbidden".to_string(),
                        details: format!("Only designated admins are allowed to send `{:#?}` transactions through this sequencer", call.discriminant()),
                    }));
                }
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
