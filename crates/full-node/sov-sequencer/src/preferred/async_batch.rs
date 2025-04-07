use std::sync::Arc;

use sov_modules_api::state::TxScratchpad;
use sov_modules_api::{
    ChangeSet, Context, DispatchCall, FullyBakedTx, IncrementalBatch, InjectedControlFlow,
    IterableBatchWithId, MaybeExecuted, NoOpControlFlow, ProvisionalSequencerOutcome, Runtime,
    TransactionReceipt, TxChangeSet, TxControlFlow,
};
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::oneshot;

use super::{RejectReason, Spec, StateCheckpoint};
use crate::common::sender_is_allowed;

/// A batch that might be received async from some producer
#[derive(Debug)]
pub enum MaybeAsyncBatch<S: Spec> {
    /// The batch is streamed from a channel.
    Async {
        txs_receiver: Receiver<FullyBakedTx>,
        responder: AsyncBatchResponder<S>,
        setup_sender: Option<oneshot::Sender<ChangeSet>>,
        address: S::Address,
    },
    /// Batch contents are fully known ahead of time.
    Sync {
        batch: IterableBatchWithId<S, NoOpControlFlow>,
    },
}

impl<S: Spec> MaybeAsyncBatch<S> {
    /// Create a new batch with a receiver for transactions.
    pub fn new_async(
        txs_receiver: Receiver<FullyBakedTx>,
        setup_sender: oneshot::Sender<ChangeSet>,
        result_channel: Sender<Result<(TransactionReceipt<S>, TxChangeSet), RejectReason>>,
        tx_profit_threshold: u128,
        sequencer_admins: Arc<Vec<S::Address>>,
        address: S::Address,
    ) -> Self {
        Self::Async {
            txs_receiver,
            address,
            setup_sender: Some(setup_sender),
            responder: AsyncBatchResponder {
                result_channel,
                admins: sequencer_admins,
                tx_profit_threshold,
            },
        }
    }

    /// Create a MaybeAsyncBatch from a batch whose contents are already known.
    pub fn new_sync(batch: IterableBatchWithId<S, NoOpControlFlow>) -> Self {
        Self::Sync { batch }
    }
}

/// The control flow injector for an async batch.
#[derive(Debug)]
pub enum MaybeAsyncBatchControlFlow<S: Spec> {
    Async(AsyncBatchResponder<S>),
    Sync,
}

impl<S: Spec> InjectedControlFlow<S> for MaybeAsyncBatchControlFlow<S> {
    fn post_tx(
        &self,
        provisional_outcome: ProvisionalSequencerOutcome<S>,
        dirty_scratchpad: TxScratchpad<S, StateCheckpoint<S>>,
    ) -> (StateCheckpoint<S>, TxControlFlow<TransactionReceipt<S>>) {
        match self {
            Self::Async(responder) => responder.post_tx(provisional_outcome, dirty_scratchpad),
            Self::Sync => <NoOpControlFlow as sov_modules_api::InjectedControlFlow<S>>::post_tx(
                &NoOpControlFlow,
                provisional_outcome,
                dirty_scratchpad,
            ),
        }
    }

    fn pre_flight<RT: Runtime<S>>(
        &self,
        runtime: &RT,
        context: &Context<S>,
        call: &<RT as DispatchCall>::Decodable,
    ) -> TxControlFlow<()> {
        match self {
            Self::Async(responder) => responder.pre_flight(runtime, context, call),
            Self::Sync => <NoOpControlFlow as InjectedControlFlow<S>>::pre_flight(
                &NoOpControlFlow,
                runtime,
                context,
                call,
            ),
        }
    }
}

/// The channel responsible for notifying an async tx submitter of the txs result
#[derive(Debug, derivative::Derivative)]
#[derivative(Clone)]
pub struct AsyncBatchResponder<S: Spec> {
    #[derivative(Clone(bound = ""))]
    result_channel: Sender<Result<(TransactionReceipt<S>, TxChangeSet), RejectReason>>,
    admins: Arc<Vec<S::Address>>,
    tx_profit_threshold: u128,
}

impl<S: Spec> AsyncBatchResponder<S> {
    fn send_item(&self, item: Result<(TransactionReceipt<S>, TxChangeSet), RejectReason>) {
        // Try a simple non-blocking send first, then fall back to blocking the runtime if that fails
        if let Err(TrySendError::Full(item)) = self.result_channel.try_send(item) {
            let _ = Handle::current().block_on(async move { self.result_channel.send(item).await });
        }
    }
}

impl<S: Spec> AsyncBatchResponder<S> {
    fn pre_flight<RT: Runtime<S>>(
        &self,
        runtime: &RT,
        context: &Context<S>,
        call: &<RT as DispatchCall>::Decodable,
    ) -> TxControlFlow<()> {
        if sender_is_allowed(
            runtime,
            call,
            context.sender(),
            context.sequencer_da_address(),
            self.admins.as_slice(),
        ) {
            TxControlFlow::ContinueProcessing(())
        } else {
            self.send_item(Err(RejectReason::SenderMustBeAdmin));
            TxControlFlow::IgnoreTx
        }
    }

    fn post_tx(
        &self,
        provisional_outcome: ProvisionalSequencerOutcome<S>,
        dirty_scratchpad: TxScratchpad<S, StateCheckpoint<S>>,
    ) -> (StateCheckpoint<S>, TxControlFlow<TransactionReceipt<S>>) {
        let ProvisionalSequencerOutcome {
            reward,
            penalty,
            execution_status,
        } = provisional_outcome;
        let MaybeExecuted::Executed(receipt) = execution_status else {
            self.send_item(Err(RejectReason::SequencerOutOfGas));
            return (dirty_scratchpad.revert(), TxControlFlow::IgnoreTx);
        };

        if !receipt.receipt.is_successful() {
            self.send_item(Ok((receipt, dirty_scratchpad.tx_changes())));
            return (dirty_scratchpad.revert(), TxControlFlow::IgnoreTx);
        }

        if penalty > reward || reward.saturating_sub(penalty) < self.tx_profit_threshold {
            self.send_item(Err(RejectReason::InsufficientReward {
                expected: self.tx_profit_threshold,
                found: reward.saturating_sub(penalty).0,
            }));
            return (dirty_scratchpad.revert(), TxControlFlow::IgnoreTx);
        }

        self.send_item(Ok((receipt.clone(), dirty_scratchpad.tx_changes())));
        (
            dirty_scratchpad.commit(),
            TxControlFlow::ContinueProcessing(receipt),
        )
    }
}

impl<S: Spec> IncrementalBatch<S> for MaybeAsyncBatch<S> {
    type ControlFlow = MaybeAsyncBatchControlFlow<S>;

    fn known_remaining_txs(&self) -> Option<usize> {
        match self {
            MaybeAsyncBatch::Async { .. } => None,
            MaybeAsyncBatch::Sync { batch } => Some(batch.batch.len()),
        }
    }

    fn id(&self) -> Option<[u8; 32]> {
        match self {
            MaybeAsyncBatch::Async { .. } => None,
            MaybeAsyncBatch::Sync { batch } => Some(batch.id),
        }
    }

    fn pre_flight(&mut self, state_checkpoint: &mut StateCheckpoint<S>) {
        match self {
            MaybeAsyncBatch::Async { setup_sender, .. } => {
                let changes = state_checkpoint.changes();
                // If the receiver is no longer available, we don't care about sending the changes.
                let _  = setup_sender.take().expect("The pre-flight hook of a single batch was invoked multiple times! This is a bug - please report it.").send(changes);
            }
            MaybeAsyncBatch::Sync { .. } => {}
        }
    }

    fn sequencer_address(&self) -> S::Address {
        match self {
            MaybeAsyncBatch::Async { address, .. } => address.clone(),
            MaybeAsyncBatch::Sync { batch } => batch.sequencer_address.clone(),
        }
    }
}

impl<S: Spec> Iterator for MaybeAsyncBatch<S> {
    type Item = (FullyBakedTx, MaybeAsyncBatchControlFlow<S>);

    fn next(&mut self) -> Option<(FullyBakedTx, MaybeAsyncBatchControlFlow<S>)> {
        // Get a handle to the current runtime, then block on receiving an update
        // from the channel. This is coupled to the implementation of the sequencer,
        // which requires that the apply_slot function be spawned on a blocking thread
        match self {
            MaybeAsyncBatch::Async {
                txs_receiver,
                responder,
                ..
            } => Handle::current()
                .block_on(txs_receiver.recv())
                .map(|item| (item, MaybeAsyncBatchControlFlow::Async(responder.clone()))),
            MaybeAsyncBatch::Sync { batch } => batch
                .next()
                .map(|(item, _)| (item, MaybeAsyncBatchControlFlow::Sync)),
        }
    }
}
