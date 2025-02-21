use std::sync::Arc;

use sov_modules_api::state::TxScratchpad;
use sov_modules_api::{
    ChangeSet, Context, DispatchCall, FullyBakedTx, IncrementalBatch, InjectedControlFlow,
    IterableBatchWithId, MaybeExecuted, NoOpControlFlow, ProvisionalSequencerOutcome, Runtime,
    TxChangeSet, TxControlFlow,
};
use sov_rollup_interface::stf::{TransactionReceipt, TxReceiptContents};
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::oneshot;

use super::{RejectReason, Spec, StateCheckpoint};
use crate::common::sender_is_allowed;

/// A batch that might be received async from some producer
#[derive(Debug)]
pub enum MaybeAsync<S: Spec> {
    /// The batch is received async from a channel
    Async((Receiver<FullyBakedTx>, <S as Spec>::Address)),
    /// The batch is already known
    // This is unused because we decided to split execution of kernel blobs into a separate PR.
    // Kernel blobs will be sync.
    #[allow(dead_code)]
    Sync(IterableBatchWithId<S>),
}

/// The control flow injector for an async batch.
#[derive(Debug)]
pub enum MaybeAsyncControlFlow<R, S: Spec> {
    Async(AsyncBatchResponder<R, S>),
    Sync,
}

impl<T: TxReceiptContents, S: Spec> InjectedControlFlow<TransactionReceipt<T>, S>
    for MaybeAsyncControlFlow<TransactionReceipt<T>, S>
{
    fn post_tx(
        &self,
        provisional_outcome: ProvisionalSequencerOutcome<TransactionReceipt<T>>,
        dirty_scratchpad: TxScratchpad<S, StateCheckpoint<S>>,
    ) -> (StateCheckpoint<S>, TxControlFlow<TransactionReceipt<T>>) {
        match self {
            Self::Async(responder) => responder.post_tx(provisional_outcome, dirty_scratchpad),
            Self::Sync => NoOpControlFlow.post_tx(provisional_outcome, dirty_scratchpad),
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
            Self::Sync => {
                <NoOpControlFlow as InjectedControlFlow<TransactionReceipt<T>, S>>::pre_flight(
                    &NoOpControlFlow,
                    runtime,
                    context,
                    call,
                )
            }
        }
    }
}

/// Contains raw transactions received from an async source in real time
#[derive(Debug)]
pub struct AsyncBatch<R, S: Spec> {
    /// The batch contents, which may be sync or async
    pub contents: MaybeAsync<S>,
    /// The channel to send responses on
    pub result_channel: Sender<Result<(R, TxChangeSet), RejectReason>>,
    /// A channel for sending the state changes from "setup" (everything before execution of the first sequencer tx)
    /// and "teardown" (everything after execution of the last sequencer tx)
    pub setup_channel: Option<oneshot::Sender<ChangeSet>>,
    /// The minimum fee that the sequencer is currently willing to earn. Txs which net
    /// less than this fee will be rejected.
    pub tx_profit_threshold: u64,
    pub sequencer_admins: Arc<Vec<S::Address>>,
}

impl<R, S: Spec> AsyncBatch<R, S> {
    /// Create a new batch with a receiver for transactions
    pub fn new_async(
        tx_receiver: Receiver<FullyBakedTx>,
        sequencer_address: <S as Spec>::Address,
        setup_channel: oneshot::Sender<ChangeSet>,
        result_channel: Sender<Result<(R, TxChangeSet), RejectReason>>,
        tx_profit_threshold: u64,
        sequencer_admins: Arc<Vec<S::Address>>,
    ) -> Self {
        Self {
            contents: MaybeAsync::Async((tx_receiver, sequencer_address)),
            result_channel,
            setup_channel: Some(setup_channel),
            tx_profit_threshold,
            sequencer_admins,
        }
    }
}

/// The channel responsible for notifying an async tx submitter of the txs result
#[derive(Debug)]
pub struct AsyncBatchResponder<R, S: Spec> {
    result_channel: Sender<Result<(R, TxChangeSet), RejectReason>>,
    admins: Arc<Vec<S::Address>>,
    tx_profit_threshold: u64,
}

impl<R, S: Spec> AsyncBatchResponder<R, S> {
    fn send_item(&self, item: Result<(R, TxChangeSet), RejectReason>) {
        // Try a simple non-blocking send first, then fall back to blocking the runtime if that fails
        if let Err(TrySendError::Full(item)) = self.result_channel.try_send(item) {
            let _ = Handle::current().block_on(async move { self.result_channel.send(item).await });
        }
    }
}

impl<T: TxReceiptContents, S: Spec> AsyncBatchResponder<TransactionReceipt<T>, S> {
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
        provisional_outcome: ProvisionalSequencerOutcome<TransactionReceipt<T>>,
        dirty_scratchpad: TxScratchpad<S, StateCheckpoint<S>>,
    ) -> (StateCheckpoint<S>, TxControlFlow<TransactionReceipt<T>>) {
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
            self.send_item(Ok((receipt, dirty_scratchpad.changes())));
            return (dirty_scratchpad.revert(), TxControlFlow::IgnoreTx);
        }

        if penalty > reward || reward - penalty < self.tx_profit_threshold {
            self.send_item(Err(RejectReason::InsufficientReward {
                expected: self.tx_profit_threshold,
                found: reward - penalty,
            }));
            return (dirty_scratchpad.revert(), TxControlFlow::IgnoreTx);
        }

        self.send_item(Ok((receipt.clone(), dirty_scratchpad.changes())));
        (
            dirty_scratchpad.commit(),
            TxControlFlow::ContinueProcessing(receipt),
        )
    }
}

impl<T: TxReceiptContents, S: Spec> IncrementalBatch<TransactionReceipt<T>, S>
    for AsyncBatch<TransactionReceipt<T>, S>
{
    type ControlFlow = MaybeAsyncControlFlow<TransactionReceipt<T>, S>;

    fn known_remaining_txs(&self) -> Option<usize> {
        use MaybeAsync::*;
        match &self.contents {
            Async(_) => None,
            Sync(batch) => Some(batch.batch.len()),
        }
    }

    fn id(&self) -> Option<[u8; 32]> {
        use MaybeAsync::*;
        match &self.contents {
            Async(_) => None,
            Sync(batch) => Some(batch.id),
        }
    }

    fn pre_flight(&mut self, state_checkpoint: &StateCheckpoint<S>) {
        let changes = state_checkpoint.changes();
        // If the receiver is no longer available, we don't care about sending the changes.
        let _  = self.setup_channel.take().expect("The pre-flight hook of a single batch was invoked multiple times! This is a bug - please report it.").send(changes);
    }

    fn sequencer_address(&self) -> S::Address {
        use MaybeAsync::*;
        match &self.contents {
            Async((_, sequencer_address)) => sequencer_address.clone(),
            Sync(batch) => batch.sequencer_address.clone(),
        }
    }
}

impl<R, S: Spec> Iterator for AsyncBatch<R, S> {
    type Item = (FullyBakedTx, MaybeAsyncControlFlow<R, S>);

    fn next(&mut self) -> Option<(FullyBakedTx, MaybeAsyncControlFlow<R, S>)> {
        use MaybeAsync::*;
        // Get a handle to the current runtime, then block on receiving an update
        // from the channel. This is coupled to the implementation of the sequencer,
        // which requires that the apply_slot function be spawned on a blocking thread
        match &mut self.contents {
            Async((receiver, _)) => Handle::current().block_on(receiver.recv()).map(|item| {
                (
                    item,
                    MaybeAsyncControlFlow::Async(AsyncBatchResponder {
                        result_channel: self.result_channel.clone(),
                        tx_profit_threshold: self.tx_profit_threshold,
                        admins: self.sequencer_admins.clone(),
                    }),
                )
            }),
            Sync(iter) => iter
                .next()
                .map(|(item, _)| (item, MaybeAsyncControlFlow::Sync)),
        }
    }
}
