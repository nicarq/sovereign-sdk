use std::sync::Arc;

use futures::StreamExt;
use sov_db::ledger_db::LedgerDb;
use sov_modules_api::rest::ApiState;
use sov_modules_api::{RawTx, RuntimeEventResponse};
use sov_rollup_interface::da::{BlockHeaderTrait, DaSpec};
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::node::ledger_api::{ItemOrHash, LedgerStateProvider, QueryMode};
use sov_rollup_interface::TxHash;
use tokio::sync::{broadcast, Mutex, MutexGuard};
use tracing::{error, info};

use super::tx_status::{TxStatus, TxStatusManager};
use super::SubmittedBatchInfo;
use crate::batch_builders::{
    AcceptTxError, AcceptedTx, BatchBuilder, DataWithEvents, FreshlyBuiltBatch,
};
use crate::drop_notifier::{DropNotification, DropNotifier};
use crate::{SeqDbTx, SequencerDb, SequencerSpec};

/// Single data structure that manages mempool and batch producing.
#[derive(Clone, derive_more::Deref)]
pub struct Sequencer<Ss: SequencerSpec> {
    #[deref(forward)]
    inner: Arc<Inner<Ss>>,
    _drop_notifier: Arc<DropNotifier>,
}

pub struct Inner<Ss: SequencerSpec> {
    batch_builder: Mutex<Ss::BatchBuilder>,
    // The sequencer's copy of the batch-builder's API state. This is
    // automatically updated by the batch-builder with the latest state.
    // We simply cache a copy so that we don't need to lock the builder to retrieve it.
    api_state: ApiState<<Ss::BatchBuilder as BatchBuilder>::Spec>,
    pub(crate) sequencer_db: SequencerDb,
    events_sender: broadcast::Sender<
        RuntimeEventResponse<
            <<Ss::BatchBuilder as BatchBuilder>::Confirmation as DataWithEvents>::EventInner,
        >,
    >,
    da_service: Ss::Da,
    tx_status_manager: TxStatusManager<<Ss::Da as DaService>::Spec>,
    automatic_batch_production: bool,
}

impl<Ss: SequencerSpec> Inner<Ss> {
    async fn build_and_send_batch(
        &self,
        da_height: u64,
        batch_builder: &mut MutexGuard<'_, Ss::BatchBuilder>,
    ) -> anyhow::Result<SubmittedBatchInfo> {
        let FreshlyBuiltBatch {
            inner: next_batch,
            hashes: tx_hashes,
        } = batch_builder.build_next_batch(da_height).await?;
        let num_txs = tx_hashes.len();
        let serialized_batch = borsh::to_vec(&next_batch)
            .expect("Failed to serialize batch inside sequencer; this is a bug, please report it");

        let fee = match self.da_service.estimate_fee(serialized_batch.len()).await {
            Ok(fee) => fee,
            Err(e) => anyhow::bail!(
                "failed to submit batch: could not determine appropriate fee rate: {}",
                e
            ),
        };
        let submit_blob_receipt = match self
            .da_service
            .send_transaction(&serialized_batch, fee)
            .await
        {
            Ok(id) => id,
            Err(e) => anyhow::bail!("failed to submit batch: {}", e),
        };

        batch_builder.clear_batch().await?;

        for tx_hash in tx_hashes {
            self.tx_status_manager.notify(
                tx_hash,
                TxStatus::Published {
                    da_tx_id: submit_blob_receipt.transaction_id.clone(),
                },
            );
        }

        Ok(SubmittedBatchInfo { da_height, num_txs })
    }

    async fn produce_batch(&self) -> anyhow::Result<()> {
        let da_height = self
            .da_service
            .get_head_block_header()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch current head: {}", e))?
            .height();

        // Acquire the lock before any DA operation, to avoid out-of-order
        // batches and other potential issues.
        let mut batch_builder = self.batch_builder.lock().await;
        self.build_and_send_batch(da_height, &mut batch_builder)
            .await?;

        Ok(())
    }
}

impl<Ss: SequencerSpec> Sequencer<Ss> {
    /// Creates a new [`Sequencer`] from a [`BatchBuilder`] and a [`DaService`].
    pub fn new(
        batch_builder: Ss::BatchBuilder,
        da_service: Ss::Da,
        tx_status_manager: TxStatusManager<<Ss::Da as DaService>::Spec>,
        sequencer_db: SequencerDb,
        ledger_db: LedgerDb,
        automatic_batch_production: bool,
    ) -> Self {
        let (events_sender, _) = broadcast::channel(100);

        let (drop_notifier, dropped) = DropNotifier::build();
        let api_state = batch_builder.api_state();
        let inner = Arc::new(Inner {
            batch_builder: Mutex::new(batch_builder),
            events_sender,
            api_state,
            sequencer_db,
            da_service,
            tx_status_manager,
            automatic_batch_production,
        });

        tokio::spawn({
            let inner = inner.clone();
            async move {
                sequencer_background_task::<Ss>(inner, ledger_db, dropped)
                    .await
                    .ok();
            }
        });

        Self {
            inner,
            _drop_notifier: Arc::new(drop_notifier),
        }
    }

    /// Returns a reference to the underlying [`SequencerDb`].
    pub fn db(&self) -> &SequencerDb {
        &self.inner.sequencer_db
    }

    /// Locks the batch builder and returns a reference to it.
    pub async fn batch_builder(&self) -> MutexGuard<Ss::BatchBuilder> {
        self.inner.batch_builder.lock().await
    }

    /// Subscribes to events emitted by the sequencer.
    pub async fn subscribe_events(
        &self,
    ) -> broadcast::Receiver<
        RuntimeEventResponse<
            <<Ss::BatchBuilder as BatchBuilder>::Confirmation as DataWithEvents>::EventInner,
        >,
    > {
        self.events_sender.subscribe()
    }

    /// Checks whether the sequencer is ready to accept transactions.
    pub async fn is_ready(&self) -> bool {
        self.batch_builder.lock().await.is_ready()
    }

    pub(crate) fn tx_status_manager(&self) -> &TxStatusManager<<Ss::Da as DaService>::Spec> {
        &self.tx_status_manager
    }

    /// Get the latest API state from the batch builder
    pub fn api_state(&self) -> ApiState<<Ss::BatchBuilder as BatchBuilder>::Spec> {
        self.api_state.clone()
    }

    /// Calls [`BatchBuilder::accept_tx`] for each transaction, and finally
    /// [`BatchBuilder::build_next_batch`].
    pub async fn submit_batch(
        &self,
        txs: Vec<<Ss::BatchBuilder as BatchBuilder>::TxInput>,
    ) -> anyhow::Result<SubmittedBatchInfo> {
        // Acquire the lock before any DA operation, to avoid out-of-order
        // batches and other potential issues.
        let mut batch_builder = self.batch_builder().await;

        let mut accept_tx_results = vec![];
        for tx in txs {
            let mut result = batch_builder.accept_tx(tx.clone()).await;

            if let Ok(accepted) = &result {
                for event in accepted.confirmation.events() {
                    self.events_sender.send(event).ok();
                }
                let stored_tx = SeqDbTx::new::<Ss::BatchBuilder>(accepted.tx_hash, tx);

                // Send notification.
                self.tx_status_manager
                    .notify(accepted.tx_hash, TxStatus::Submitted);
                if let Err(e) = self.sequencer_db.insert(&stored_tx).await {
                    error!(%e, "Database error. Failed to add transaction to batch");
                    result = Err(AcceptTxError {
                        http_status: 500,
                        title: "Database Error".to_string(),
                        details: String::new(),
                    });
                }
            }

            accept_tx_results.push(result);
        }

        tracing::info!("Submit batch request has been received!");

        let da_height = self
            .da_service
            .get_head_block_header()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch current head: {}", e))?
            .height();

        self.inner
            .build_and_send_batch(da_height, &mut batch_builder)
            .await
    }

    /// Encodes the transaction into the format accepted by [`BatchBuilder::accept_tx`].
    ///
    /// TODO(@neysofu): this method should be replaced an API endpoint -aware
    /// approach, so that multiple transaction formats can be supported.
    pub fn encode_tx(&self, raw: RawTx) -> <Ss::BatchBuilder as BatchBuilder>::TxInput {
        Ss::BatchBuilder::encode_tx(raw)
    }

    /// See [`BatchBuilder::accept_tx`].
    pub async fn accept_tx(
        &self,
        tx_input: <Ss::BatchBuilder as BatchBuilder>::TxInput,
    ) -> Result<AcceptedTx<<Ss::BatchBuilder as BatchBuilder>::Confirmation>, AcceptTxError> {
        let mut batch_builder = self.batch_builder().await;

        let tx_bytes = borsh::to_vec(&tx_input).expect("Failed to serialize transaction");
        tracing::info!(tx = hex::encode(&tx_bytes), "Accepting transaction");
        let accepted = batch_builder.accept_tx(tx_input.clone()).await?;

        for event in accepted.confirmation.events() {
            self.events_sender.send(event).ok();
        }

        let stored_tx = SeqDbTx::new::<Ss::BatchBuilder>(accepted.tx_hash, tx_input);
        self.sequencer_db.insert(&stored_tx).await.map_err(|e| {
            error!(%e, "Database error. Failed to accept transaction");
            AcceptTxError {
                http_status: 500,
                title: "Database Error".to_string(),
                details: String::new(),
            }
        })?;
        self.tx_status_manager
            .notify(accepted.tx_hash, TxStatus::Submitted);

        Ok(accepted)
    }

    /// Queries the latest known status of the given transaction. Best-effort,
    /// can't promise to always know the status.
    pub async fn tx_status(
        &self,
        tx_hash: &TxHash,
    ) -> anyhow::Result<Option<TxStatus<<<Ss::Da as DaService>::Spec as DaSpec>::TransactionId>>>
    {
        // TODO: This report is not completely accurate. The mempool is allowed to drop transactions
        // but currently has no mechanism to remove them from the sequencer_db, so there can be a window
        // between the time that a tx is evicted from the notificaiton cache and the time its entry is
        // TTL'd where it will report `Submitted` instead of `Dropped`
        if let Some(status) = self.tx_status_manager.get_cached(tx_hash) {
            return Ok(Some(status));
        } else if self.sequencer_db.contains_tx(tx_hash).await? {
            return Ok(Some(TxStatus::Submitted));
        }
        Ok(None)
    }
}

pub async fn sequencer_background_task<Ss: SequencerSpec>(
    inner: Arc<Inner<Ss>>,
    ledger_db: LedgerDb,
    mut drop_notification: DropNotification,
) -> anyhow::Result<()> {
    let mut sub = ledger_db.subscribe_slots();
    let mut storage_receiver = inner.batch_builder.lock().await.storage_receiver();

    loop {
        tokio::select! {
            _ = &mut drop_notification => {
                info!("Sequencer was dropped, stopping listener for new slots");
                break;
            },
            changed = storage_receiver.changed() => {
                if changed.is_err() {
                    continue;
                }

                // Update storage.
                let storage = storage_receiver.borrow().clone();
                inner.batch_builder.lock().await.set_state(0, storage).await;

                if inner.automatic_batch_production {
                    inner.produce_batch().await?;
                }
            },
            slot_number_opt = sub.next() => {
                if let Some(slot_number) = slot_number_opt {
                    notify_processed_slot::<Ss>(inner.clone(), &ledger_db,  slot_number).await?;
                } else {
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn notify_processed_slot<Ss: SequencerSpec>(
    inner: Arc<Inner<Ss>>,
    ledger_db: &LedgerDb,
    slot_number: u64,
) -> anyhow::Result<()> {
    let slot = ledger_db
        .get_slot_by_number::<Ss::BatchReceipt, Ss::TxReceipt, Ss::Event>(
            slot_number,
            QueryMode::Full,
        )
        .await?
        .unwrap();
    for batch in slot.batches.unwrap_or_default().iter() {
        let ItemOrHash::Full(batch) = batch else {
            continue;
        };
        for tx in batch.txs.as_deref().unwrap_or_default().iter() {
            let ItemOrHash::Full(tx) = tx else {
                continue;
            };

            let tx_hash = TxHash::new(tx.hash);

            inner.tx_status_manager.notify(tx_hash, TxStatus::Processed);
        }
    }

    Ok(())
}
