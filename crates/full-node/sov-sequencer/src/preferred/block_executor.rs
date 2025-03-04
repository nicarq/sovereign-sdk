use std::mem::replace;
use std::sync::Arc;

use anyhow::Context;
use axum::http::StatusCode;
use sov_modules_api::capabilities::{BlobSelector, BlobSelectorOutput, ChainState, FatalError};
use sov_modules_api::{
    BlobDataWithId, ExecutionContext, FullyBakedTx, Gas, GasSpec, KernelStateAccessor,
    NestedEnumUtils, RejectReason, Runtime, SelectedBlob, Spec, StateCheckpoint, VersionReader,
    VisibleSlotNumber,
};
use sov_modules_stf_blueprint::{StfBlueprint, TransactionReceipt};
use sov_rest_utils::{json_obj, ErrorObject};
use sov_state::{Namespace, StateRoot, StateUpdate, Storage};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{self, Sender};
use tokio::sync::oneshot;
use tracing::trace;

use super::{
    BackgroundTaskState, InternalState, PreferredBatchToRestore, PreferredSequencerConfig,
    VisibleSlotNumberIncrease,
};
use crate::common::generic_accept_tx_error;
use crate::preferred::async_batch::AsyncBatch;
use crate::SequencerConfig;

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

pub(crate) struct RollupBlockExecutor<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    state: InternalState<S>,
    runtime: Rt,
    config: SequencerConfig<S::Da, S::Address, PreferredSequencerConfig>,
    // A sender notifying that this acceptor has successfully shut down. We give a handle to
    // each background task when it is spawned, ensuring that this channel remains open as long
    // as any background task is operational even if the acceptor is dropped.
    shutdown_notifier: Sender<()>,
}

impl<S: Spec, Rt: Runtime<S>> RollupBlockExecutor<S, Rt> {
    /// The maximum number of transactions that can be buffered before incoming txs start getting
    /// rejected.
    const MAX_BUFFERED_TXS: usize = 1;

    pub fn new(
        state: InternalState<S>,
        config: SequencerConfig<S::Da, S::Address, PreferredSequencerConfig>,
        shutdown_notifier: Sender<()>,
    ) -> RollupBlockExecutor<S, Rt> {
        RollupBlockExecutor {
            state,
            config,
            shutdown_notifier,
            runtime: Default::default(),
        }
    }

    pub fn state(&self) -> &InternalState<S> {
        &self.state
    }

    pub fn consume(self) -> InternalState<S> {
        self.state
    }

    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn replace_state(&mut self, state: InternalState<S>) {
        let current_state = replace(&mut self.state, InternalState::Placeholder);
        if let InternalState::InProgressBatch { task_state, .. } = current_state {
            task_state.shutdown().abort();
        }

        self.state = state;
    }

    /// Calls to this method must happen "between"
    /// [`Self::start_rollup_block`] and
    /// [`Self::end_rollup_block_if_in_progress`].
    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn apply_tx_to_in_progress_batch(
        &mut self,
        baked_tx: &FullyBakedTx,
    ) -> Result<TransactionReceipt<S>, RollupBlockExecutorError<S>> {
        let InternalState::InProgressBatch {
            mut checkpoint,
            mut task_state,
        } = replace(&mut self.state, InternalState::Placeholder)
        else {
            panic!("Accepting a transaction, yet there's no in-progress batch ({:?}). This is a bug in the sequencer, please report it.", self.state);
        };

        let result = self
            .apply_tx_to_in_progress_batch_inner(baked_tx, &mut checkpoint, &mut task_state)
            .await;

        self.state = InternalState::InProgressBatch {
            checkpoint,
            task_state,
        };

        result
    }

    async fn apply_tx_to_in_progress_batch_inner(
        &mut self,
        baked_tx: &FullyBakedTx,
        checkpoint: &mut StateCheckpoint<S>,
        task_state: &mut BackgroundTaskState<S>,
    ) -> Result<TransactionReceipt<S>, RollupBlockExecutorError<S>> {
        let (call, _) = Rt::decode_serialized_tx(&self.runtime, baked_tx)?;

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
                call: format!("{:?}", call.discriminant()),
            })?;

        if !receipt.receipt.is_successful() {
            return Err(RollupBlockExecutorError::UnsuccessfulTransaction { receipt });
        }

        checkpoint.apply_changes(change_set.0);

        Ok(receipt)
    }

    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn replay_batch(
        &mut self,
        batch: &PreferredBatchToRestore,
        node_state_root: &<S::Storage as Storage>::Root,
    ) -> anyhow::Result<()> {
        assert!(
            matches!(self.state, InternalState::Idle { .. }),
            "Replaying a preferred batch, but the state is invalid and doesn't allow it ({:?}). This is a bug, please report it.",
            self.state
        );

        trace!(
            num_txs = batch.batch.inner.data.len(),
            "Re-applying batch state changes"
        );

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

        trace!("Replaying txs");

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
                tracing::error!(
                    "Transaction was soft-confirmed but failed to be re-applied; this is a bug, please report it {:?}",
                    err
                );
                std::process::exit(1);
            }
        }

        trace!("Done replaying txs");

        if !batch.is_in_progress {
            self.end_rollup_block_if_in_progress().await;
        } else {
            trace!("The batch is still in progress; will keep the background task running");
        }

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn start_rollup_block(
        &mut self,
        visible_increase: VisibleSlotNumberIncrease,
        visible_slot_number_after_increase: Option<VisibleSlotNumber>,
        // We pass the node state root explicitly because retrieving it is
        // fallible, so it's convenient to front-load the error-checking.
        node_state_root: &<S::Storage as Storage>::Root,
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

    #[tracing::instrument(skip_all, level = "trace")]
    async fn start_rollup_block_inner(
        &mut self,
        visible_increase: VisibleSlotNumberIncrease,
        visible_slot_number_after_increase: Option<VisibleSlotNumber>,
        node_state_root: &<S::Storage as Storage>::Root,
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

        let prev_state_root = prev_state_root_opt.as_ref().unwrap_or(node_state_root);
        let user_state_root = prev_state_root.namespace_root(sov_state::ProvableNamespace::User);

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
                    &user_state_root,
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

                tracing::trace!(
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

    #[tracing::instrument(skip_all, level = "trace")]
    pub(crate) async fn end_rollup_block_if_in_progress(&mut self) {
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

        let (state_root, changes) = task_state.shutdown().await.expect(
            "Transaction acceptor task failed unexpectedly! This is a bug, please report it.",
        );

        checkpoint.apply_changes(changes);

        self.state = InternalState::Idle {
            checkpoint,
            prev_state_root_opt: Some(state_root),
        };

        trace!("Successfully ended rollup block");
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
