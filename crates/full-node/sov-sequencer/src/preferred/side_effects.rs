use std::collections::VecDeque;

use sov_blob_storage::SequenceNumber;
use sov_modules_api::{Runtime, Spec, StateCheckpoint, TxChangeSet};
use sov_rollup_interface::node::da::DaService;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{error, trace, warn};

use super::executor_events::ExecutorEvent;
use crate::common::AcceptedTx;
use crate::preferred::transaction_subscriptions::TxResultWriter;
use crate::preferred::{
    exit_rollup, track_in_progress_batch_size, Confirmation, PreferredBatchToReplay,
    PreferredBlobSender, PreferredSequencerDb, PreferredSequencerReadBatch,
    PreferredSequencerReadBlob, RecoveryStrategy, RECOVERY_ERROR_MESSAGE_ON_NONE_STRATEGY,
};

type AcceptedTxContents<S, Rt> = (
    AcceptedTx<Confirmation<S, Rt>>,
    TxChangeSet,
    oneshot::Sender<AcceptedTx<Confirmation<S, Rt>>>,
);

/// A task that runs in the background and handles side effects of accepted transactions.
pub(super) struct SideEffectsTask<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    pub checkpoint_sender: watch::Sender<StateCheckpoint<S>>,
    pub blob_sender: PreferredBlobSender<Da>,
    pub db: PreferredSequencerDb<S, Rt>,
    pub executor_events_receiver: mpsc::Receiver<ExecutorEvent<S, Rt>>,
    pub shutdown_sender: watch::Sender<()>,
    pub transaction_cache: TxResultWriter<S, Rt>,
}

impl<S, Rt, Da> SideEffectsTask<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    /// Syncs [`ApiState`]s with the latest [`StateCheckpoint`].
    #[tracing::instrument(skip_all, level = "trace")]
    fn update_api_state(&self, checkpoint: StateCheckpoint<S>) {
        if self.checkpoint_sender.send(checkpoint).is_err() {
            tracing::debug!("Could not send checkpoint because the receiver has been dropped; this probably means the rollup is shutting down");
        }
    }

    /// Applies the changes to the current [`StateCheckpoint`].
    #[tracing::instrument(skip_all, level = "trace")]
    fn update_api_state_with_changes(&self, changes: TxChangeSet) {
        self.checkpoint_sender.send_modify(|checkpoint| {
            checkpoint.apply_changes(changes.0);
        });
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn close_and_publish_current_batch(
        &mut self,
        checkpoint: StateCheckpoint<S>,
    ) -> anyhow::Result<()> {
        let batch = self.db.terminate_batch().await?;
        self.update_api_state(checkpoint);

        // Publish the batch.
        self.blob_sender
            .hooks()
            .add_txs(batch.blob_id, batch.tx_hashes.clone())
            .await;
        self.blob_sender.publish_batch(batch).await?;

        // Update the metrics.
        track_in_progress_batch_size(
            self.db
                .in_progress_batch_opt()
                .map(|b| b.txs.len() as u64)
                .unwrap_or(0),
        );
        Ok(())
    }

    async fn trigger_recovery(
        &mut self,
        next_sequence_number_according_to_node: SequenceNumber,
        recovery_strategy: RecoveryStrategy,
    ) -> anyhow::Result<()> {
        let batches_to_replay =
            self.fetch_completed_blobs_by_sequence(next_sequence_number_according_to_node, true);
        if !batches_to_replay.is_empty() {
            match recovery_strategy {
                RecoveryStrategy::TryToSave => {
                    // Flush our batches to try to save them if we can
                    warn!(num_batches_to_replay = batches_to_replay.len(), "TryToSave recovery strategy has been configured. The currently pending soft confirmations will be flushed to the node. This may save some of the transactions, but if any are no longer valid, the sequencer will be penalised.");
                    self.flush_pending_batches_for_recovery(next_sequence_number_according_to_node)
                        .await?;
                }
                RecoveryStrategy::None => {
                    // Shut down
                    error!(RECOVERY_ERROR_MESSAGE_ON_NONE_STRATEGY);
                    exit_rollup(&self.shutdown_sender).await;
                }
            }
        } else {
            warn!("Recovery: sequencer will now fast-forward the visible slot number, and resume normal operations when ready. There were no pending soft confirmations, so users will not be affected except for the downtime.");
        }
        Ok(())
    }

    async fn flush_pending_batches_for_recovery(
        &mut self,
        next_sequence_number_according_to_node: SequenceNumber,
    ) -> anyhow::Result<()> {
        tracing::trace!("Recovery: flushing all preferred sequencer batches");

        // 1. close the in-progress batch, if any
        if self.db.in_progress_batch_opt().is_some() {
            tracing::debug!("Recovery: In-progress batch found, terminating it.");
            self.db.terminate_batch().await?;
            // No need to update API state, we're going to overwrite it with the node's state soon
        } else {
            tracing::debug!("Recovery: No in-progress batch to terminate.");
        }

        // 2. Flush all batches to the BlobSender
        let blobs_to_flush = self
            .db
            .all_completed_blobs_greater_than_or_equal_to(next_sequence_number_according_to_node);
        self.blob_sender.publish_blobs(blobs_to_flush).await?;

        Ok(())
    }

    fn fetch_completed_blobs_by_sequence(
        &mut self,
        after_and_including: SequenceNumber,
        include_in_progress_batch: bool,
    ) -> Vec<PreferredBatchToReplay> {
        let blobs_to_apply = self
            .db
            .all_completed_blobs_greater_than_or_equal_to(after_and_including);
        let first_sequence_number = blobs_to_apply.first().map(|b| b.sequence_number());

        trace!(
            blobs_count = blobs_to_apply.len(),
            first_sequence_number,
            last_sequence_number = blobs_to_apply.last().map(|b| b.sequence_number()),
            "Extracted blobs to apply from database"
        );

        let maybe_in_progress_batch = if include_in_progress_batch {
            self.db.in_progress_batch_opt().cloned().map(|batch| {
                let batch: PreferredSequencerReadBatch = batch.into();
                PreferredBatchToReplay {
                    is_in_progress: true,
                    visible_slot_number_after_increase: batch.visible_slot_number_after_increase,
                    batch: batch.into_with_cached_tx_hashes(),
                }
            })
        } else {
            None
        };

        blobs_to_apply
            .into_iter()
            .filter_map(|blob| match blob {
                PreferredSequencerReadBlob::Batch(batch) => Some(PreferredBatchToReplay {
                    is_in_progress: false,
                    visible_slot_number_after_increase: batch.visible_slot_number_after_increase,
                    batch: batch.into_with_cached_tx_hashes(),
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
            .chain(maybe_in_progress_batch)
            .collect::<Vec<_>>()
    }

    /// Drains at least one event from the queue, batching operations when possible.
    async fn handle_executor_event(
        &mut self,
        event_queue: &mut VecDeque<ExecutorEvent<S, Rt>>,
    ) -> Result<(), anyhow::Error> {
        let next_event = event_queue
            .pop_front()
            .expect("Tried to pop from empty event queue. This is a bug, please report it");
        match next_event {
            ExecutorEvent::AcceptedTx(accepted_tx, tx_changes, oneshot_sender) => {
                let txs_to_insert = drain_consecutive_accepted_txs(
                    (accepted_tx, tx_changes, oneshot_sender),
                    event_queue,
                );
                if tracing::enabled!(tracing::Level::DEBUG) {
                    for (tx, _, _) in txs_to_insert.iter() {
                        tracing::debug!(tx_hatsh = %tx.tx_hash, "Transaction was accepted by the sequencer");
                    }
                }
                self.db
                    .bulk_insert_txs(
                        txs_to_insert
                            .iter()
                            .map(|(tx, _, _)| (tx.tx.clone(), tx.tx_hash))
                            .collect(),
                    )
                    .await?;
                track_in_progress_batch_size(
                    self.db
                        .in_progress_batch_opt()
                        .map(|b| b.txs.len() as u64)
                        .unwrap_or(0),
                );
                for (tx, tx_changes, oneshot_sender) in txs_to_insert {
                    self.transaction_cache.insert(tx.clone()).await;
                    // If the receiver is no longer listening, just don't send the confirmation.
                    let _ = oneshot_sender.send(tx);
                    self.update_api_state_with_changes(tx_changes);
                }
            }
            ExecutorEvent::CloseBatch(checkpoint) => {
                self.close_and_publish_current_batch(checkpoint).await?;
            }
            ExecutorEvent::Flush(id) => {
                self.db.flush(id).await;
            }
            ExecutorEvent::StartBatch {
                visible_slot_number_after_increase,
                visible_slots_to_advance,
                sequence_number,
                new_checkpoint,
            } => {
                self.db
                    .start_batch(
                        visible_slot_number_after_increase,
                        visible_slots_to_advance,
                        sequence_number,
                    )
                    .await?;
                self.update_api_state(new_checkpoint);
            }
            ExecutorEvent::EnterRecoveryMode {
                recovery_strategy,
                next_sequence_number_according_to_node,
            } => {
                self.trigger_recovery(next_sequence_number_according_to_node, recovery_strategy)
                    .await?;
            }
            ExecutorEvent::PublishProofBlob(blob_id, data, sequence_number) => {
                self.db
                    .insert_proof_blob(blob_id, data.clone(), sequence_number)
                    .await?;
                self.blob_sender
                    .publish_proof(data, sequence_number, blob_id)
                    .await?;
            }
            ExecutorEvent::ForceUpdateApiState(new_checkpoint) => {
                self.update_api_state(new_checkpoint);
            }
            ExecutorEvent::PruneDb(sequence_number) => {
                self.db.prune(sequence_number).await?;
            }
            ExecutorEvent::InsertTxWithoutConfirmation(tx, tx_hash) => {
                self.db.insert_tx(tx, tx_hash).await?;
            }

            ExecutorEvent::FetchCompletedBlobs {
                after_and_including,
                oneshot_sender,
                include_in_progress_batch,
            } => {
                let blobs = self.fetch_completed_blobs_by_sequence(
                    after_and_including,
                    include_in_progress_batch,
                );
                let _ = oneshot_sender.send(blobs);
            }
            ExecutorEvent::FetchInProgressBatch(oneshot_sender) => {
                let in_progress_batch = self.db.in_progress_batch_opt().cloned().map(|b| b.into());
                let _ = oneshot_sender.send(in_progress_batch);
            }
            ExecutorEvent::UpdateStateForRecovery(checkpoint) => {
                self.update_api_state(checkpoint);
            }
            ExecutorEvent::SubscribeToEvents(sender) => {
                self.db.subscribe_to_events(sender);
            }
            ExecutorEvent::FlushTransactionsCache {
                next_tx_number,
                oneshot_sender,
            } => {
                self.transaction_cache
                    .clean_and_overwrite_next_tx_number(next_tx_number)
                    .await;
                let _ = oneshot_sender.send(());
            }
        }
        Ok(())
    }

    async fn receive_and_process_events(
        &mut self,
        mut event_queue: VecDeque<ExecutorEvent<S, Rt>>,
        max_queue_size: usize,
    ) {
        while let Some(event) = self.executor_events_receiver.recv().await {
            event_queue.push_back(event);
            while event_queue.len() < max_queue_size {
                if let Ok(event) = self.executor_events_receiver.try_recv() {
                    event_queue.push_back(event);
                } else {
                    break;
                }
            }

            while !event_queue.is_empty() {
                if let Err(e) = self.handle_executor_event(&mut event_queue).await {
                    tracing::error!("Error handling executor event: {:?}", e);
                    // If we've arleady started shutting down, this might fail - but then we're happy.
                    let _ = self.shutdown_sender.send(());
                    break;
                }
            }
        }
    }

    pub(crate) fn spawn(mut self) -> JoinHandle<()> {
        // We use a queue so that we can batch insert txs.
        let max_queue_size = self.executor_events_receiver.max_capacity();
        let event_queue = VecDeque::with_capacity(max_queue_size);
        tokio::spawn(async move {
            self.receive_and_process_events(event_queue, max_queue_size)
                .await;
        })
    }
}

fn drain_consecutive_accepted_txs<S: Spec, Rt: Runtime<S>>(
    first_tx: AcceptedTxContents<S, Rt>,
    event_queue: &mut VecDeque<ExecutorEvent<S, Rt>>,
) -> Vec<AcceptedTxContents<S, Rt>> {
    let mut txs_to_insert = vec![first_tx];
    while let Some(next_event) = event_queue.pop_front() {
        if let ExecutorEvent::AcceptedTx(accepted_tx, tx_changes, oneshot_sender) = next_event {
            txs_to_insert.push((accepted_tx, tx_changes, oneshot_sender));
        } else {
            // Otherwise, put the event back and return.
            event_queue.push_front(next_event);
            break;
        }
    }
    txs_to_insert
}

#[cfg(test)]
mod tests {
    use sov_modules_api::{
        ApiTxEffect, ChangeSet, FullyBakedTx, Gas, SuccessfulTxContents, TxHash,
    };
    use sov_test_utils::{generate_optimistic_runtime, TestSpec as S};

    generate_optimistic_runtime!(TestRuntime <= );

    use super::*;

    fn create_accepted_tx_event(number: u64) -> ExecutorEvent<S, TestRuntime<S>> {
        let tx = FullyBakedTx::new(vec![]);
        let tx_hash = TxHash::new([number as u8; 32]);
        let tx_changes = TxChangeSet(ChangeSet::new(vec![]));
        let (sender, _) = oneshot::channel();
        let confirmation = Confirmation {
            events: vec![],
            receipt: ApiTxEffect::Successful {
                data: SuccessfulTxContents {
                    gas_used: <<S as Spec>::Gas>::zero(),
                },
            },
            tx_number: number,
        };
        ExecutorEvent::AcceptedTx(
            AcceptedTx {
                tx,
                tx_hash,
                confirmation,
            },
            tx_changes,
            sender,
        )
    }

    fn split_event(
        event: ExecutorEvent<S, TestRuntime<S>>,
    ) -> AcceptedTxContents<S, TestRuntime<S>> {
        match event {
            ExecutorEvent::AcceptedTx(accepted_tx, tx_changes, oneshot_sender) => {
                (accepted_tx, tx_changes, oneshot_sender)
            }
            _ => panic!("Expected AcceptedTx event"),
        }
    }

    #[tokio::test]
    async fn test_drain_consecutive_accepted_txs() {
        let events = (1..1001)
            .map(create_accepted_tx_event)
            .collect::<VecDeque<_>>();
        // Test 1: draining from an empty queue
        {
            let first_event = create_accepted_tx_event(0);
            let drained_txs =
                drain_consecutive_accepted_txs(split_event(first_event), &mut VecDeque::new());
            // We should get back the event that we passed and no others. The queue should still be empty.
            assert_eq!(drained_txs.len(), 1);
        }

        // Test draining from a queue where the first event is not AcceptedTx
        {
            let first_event = create_accepted_tx_event(0);
            let mut event_queue = vec![ExecutorEvent::PruneDb(0)].into();
            let drained_txs =
                drain_consecutive_accepted_txs(split_event(first_event), &mut event_queue);
            // We should get back the event that we passed and no others. The queue should be untouched
            assert_eq!(drained_txs.len(), 1);
            assert_eq!(event_queue.len(), 1);
        }

        // Test draining from a queue where the first event is not AcceptedTx and there are other events in the queue
        {
            let first_event = create_accepted_tx_event(0);
            let second_event = create_accepted_tx_event(1);
            let mut event_queue = vec![ExecutorEvent::PruneDb(0), second_event].into();
            let drained_txs =
                drain_consecutive_accepted_txs(split_event(first_event), &mut event_queue);
            // We should get back the event that we passed and no others. The queue should be untouched
            assert_eq!(drained_txs.len(), 1);
            assert_eq!(event_queue.len(), 2);
        }

        // test a large queue size
        {
            let first_event = create_accepted_tx_event(0);
            let mut event_queue = events;
            // Put a non-accepted tx in the middle of the queue
            event_queue.insert(500, ExecutorEvent::PruneDb(0));
            let drained_txs =
                drain_consecutive_accepted_txs(split_event(first_event), &mut event_queue);
            // We should drain events at index 0..499 (so 500 of them) plus the "first" event
            assert_eq!(drained_txs.len(), 501);
            assert_eq!(event_queue.len(), 501); // There should be 501 events in the queue - one prune and the remaining 500 accept txs
            for i in 0..501 {
                assert_eq!(drained_txs[i as usize].0.confirmation.tx_number, i);
            }

            // Drain the prune event
            event_queue.pop_front();

            // Drain the last half of the events and check correctness
            let drained_txs = drain_consecutive_accepted_txs(
                split_event(create_accepted_tx_event(0)),
                &mut event_queue,
            );
            assert_eq!(drained_txs.len(), 501);
            assert_eq!(event_queue.len(), 0);
            let first_event_received = drained_txs.first().unwrap();
            assert_eq!(first_event_received.0.confirmation.tx_number, 0);
            for i in 1..501 {
                assert_eq!(drained_txs[i as usize].0.confirmation.tx_number, i + 500);
            }
        }
    }
}
