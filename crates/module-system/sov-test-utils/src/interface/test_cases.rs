use sov_mock_da::MockDaSpec;
use sov_modules_api::capabilities::FatalError;
use sov_modules_api::{Module, ModuleError, Spec, StateCheckpoint, TxEffect};
use sov_modules_stf_blueprint::{Runtime, SkippedReason, TxReceiptContents};

use super::messages::{BatchMessages, MessageType};
use super::{BatchExpectedReceipt, BatchSequencerOutcome};
use crate::runtime::wrapper::EndSlotClosure;
use crate::runtime::WorkingSetClosure;

/// Defines a test case at the slot level. This can be used to describe a rollup's test. It contains a list of [`BatchTestCase`]s and a post slot hook closure to
/// be run after the slot has been executed.
///
/// ## Note
/// This struct implements [`From<Vec<TxTestCase<RT, M, S>>>`] to create a [`SlotTestCase`] from a list of [`TxTestCase`]s.
/// This is useful when you want to create a [`SlotTestCase`] with a single batch filled with transactions and without a post slot hook closure.
pub struct SlotTestCase<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> {
    /// The list of [`BatchTestCase`]s to be executed in the slot.
    pub batch_test_cases: Vec<BatchTestCase<RT, M, S>>,
    /// The post slot hook closure to be executed after the slot has been executed.
    pub post_hook: EndSlotClosure<StateCheckpoint<S>>,
}

impl<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> SlotTestCase<RT, M, S> {
    /// Creates an empty [`SlotTestCase`].
    pub fn empty() -> Self {
        Self {
            batch_test_cases: vec![],
            post_hook: Box::new(|_| {}),
        }
    }

    /// Creates a [`SlotTestCase`] from a list of [`TxTestCase`]s for a batch having the outcome [`BatchSequencerOutcome::Rewarded`].
    pub fn from_rewarded_batch(tx_test_cases: Vec<TxTestCase<RT, M, S>>) -> Self {
        Self::from_batch_with_outcome(tx_test_cases, BatchSequencerOutcome::Rewarded)
    }

    /// Creates a [`SlotTestCase`] from a list of [`TxTestCase`]s for a batch having the outcome [`BatchSequencerOutcome::Slashed`].
    pub fn from_slashed_batch(
        tx_test_cases: Vec<TxTestCase<RT, M, S>>,
        reason: FatalError,
    ) -> Self {
        Self::from_batch_with_outcome(tx_test_cases, BatchSequencerOutcome::Slashed(reason))
    }

    /// Creates a [`SlotTestCase`] from a list of [`TxTestCase`]s for a batch having the outcome [`BatchSequencerOutcome::Ignored`].
    pub fn from_ignored_batch(tx_test_cases: Vec<TxTestCase<RT, M, S>>, reason: String) -> Self {
        Self::from_batch_with_outcome(tx_test_cases, BatchSequencerOutcome::Ignored(reason))
    }

    /// Creates a [`SlotTestCase`] from a list of [`TxTestCase`]s for a batch having the outcome [`BatchSequencerOutcome::NotRewardable`].
    pub fn from_not_rewardable_batch(tx_test_cases: Vec<TxTestCase<RT, M, S>>) -> Self {
        Self::from_batch_with_outcome(tx_test_cases, BatchSequencerOutcome::NotRewardable)
    }

    /// Creates a [`SlotTestCase`] from a list of [`TxTestCase`]s for a batch having the outcome `batch_outcome`.
    pub fn from_batch_with_outcome(
        tx_test_cases: Vec<TxTestCase<RT, M, S>>,
        batch_outcome: BatchSequencerOutcome,
    ) -> Self {
        Self {
            batch_test_cases: vec![BatchTestCase {
                tx_test_cases,
                outcome: batch_outcome,
            }],
            post_hook: Box::new(|_| {}),
        }
    }

    /// Converts a list of [`BatchTestCase`] into a [`SlotTestCase`] without any post-hook.
    pub fn from_batches(batches: Vec<BatchTestCase<RT, M, S>>) -> Self {
        SlotTestCase {
            batch_test_cases: batches,
            post_hook: Box::new(|_| {}),
        }
    }
}

/// Defines a test case at the batch level. This can be used to describe a rollup's test. It contains a list of [`TxTestCase`]s.
pub struct BatchTestCase<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> {
    tx_test_cases: Vec<TxTestCase<RT, M, S>>,
    outcome: BatchSequencerOutcome,
}

impl<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> BatchTestCase<RT, M, S> {
    /// Creates a new rewarded [`BatchTestCase`].
    pub fn rewarded(tx_test_cases: Vec<TxTestCase<RT, M, S>>) -> Self {
        Self::with_outcome(tx_test_cases, BatchSequencerOutcome::Rewarded)
    }

    /// Creates a new slashed [`BatchTestCase`].
    pub fn slashed(tx_test_cases: Vec<TxTestCase<RT, M, S>>, reason: FatalError) -> Self {
        Self::with_outcome(tx_test_cases, BatchSequencerOutcome::Slashed(reason))
    }

    /// Creates a new ignored [`BatchTestCase`].
    pub fn ignored(tx_test_cases: Vec<TxTestCase<RT, M, S>>, ignored_reason: String) -> Self {
        Self::with_outcome(
            tx_test_cases,
            BatchSequencerOutcome::Ignored(ignored_reason),
        )
    }

    /// Creates a new not rewardable [`BatchTestCase`].
    pub fn not_rewardable(tx_test_cases: Vec<TxTestCase<RT, M, S>>) -> Self {
        Self::with_outcome(tx_test_cases, BatchSequencerOutcome::NotRewardable)
    }

    /// Creates a new [`BatchTestCase`] with a custom outcome.
    pub fn with_outcome(
        tx_test_cases: Vec<TxTestCase<RT, M, S>>,
        outcome: BatchSequencerOutcome,
    ) -> Self {
        Self {
            tx_test_cases,
            outcome,
        }
    }

    /// Splits a [`BatchTestCase`] into a list of [`MessageType`], closures to be executed in the post_dispatch_hook, and an expected [`BatchExpectedReceipt`].
    /// We are
    pub fn split(
        self,
    ) -> (
        BatchMessages<M, S>,
        Vec<WorkingSetClosure<RT>>,
        BatchExpectedReceipt,
    ) {
        let (messages_and_post_dispatch_closures, maybe_expected_tx_receipts): (Vec<_>, Vec<_>) =
            self.tx_test_cases
                .into_iter()
                .map(|tx_test_case| match tx_test_case {
                    TxTestCase::Applied {
                        message,
                        post_dispatch_hook,
                    } => (
                        (message, Some(post_dispatch_hook)),
                        Some(TxEffect::Successful(())),
                    ),
                    TxTestCase::Reverted { message, reason } => {
                        ((message, None), Some(TxEffect::Reverted(reason)))
                    }
                    TxTestCase::Skipped {
                        message,
                        skipped_reason,
                    } => ((message, None), Some(TxEffect::Skipped(skipped_reason))),
                    TxTestCase::Dropped(message) => ((message, None), None),
                })
                .unzip();

        let batch_receipt = BatchExpectedReceipt {
            tx_receipts: maybe_expected_tx_receipts.into_iter().flatten().collect(),
            batch_outcome: self.outcome,
        };

        let (messages, post_dispatch_closures): (Vec<_>, Vec<_>) =
            messages_and_post_dispatch_closures.into_iter().unzip();

        (
            messages,
            post_dispatch_closures.into_iter().flatten().collect(),
            batch_receipt,
        )
    }
}

/// Defines a test case at the transaction level.
pub enum TxTestCase<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> {
    /// The transaction should be applied successfully and the `post_dispatch_hook` should be executed.
    Applied {
        /// The message to be sent to the runtime.
        message: MessageType<M, S>,
        /// A post_dispatch_hook closure to be executed if the transaction is applied successfully.
        post_dispatch_hook: WorkingSetClosure<RT>,
    },
    /// The transaction should be reverted.
    Reverted {
        /// The message to be sent to the runtime.
        message: MessageType<M, S>,
        /// The reason why the transaction should be reverted.
        reason: ModuleError,
    },
    /// The transaction should be skipped. Ie, the transaction's ID has been computed and a receipt was emitted but
    /// the transaction was never executed.
    Skipped {
        /// The message to be sent to the runtime.
        message: MessageType<M, S>,
        /// The reason why the transaction should be skipped.
        skipped_reason: SkippedReason,
    },
    /// The transaction should be dropped from the batch. Ie, the transaction should not generate a receipt.
    Dropped(MessageType<M, S>),
}

impl<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> TxTestCase<RT, M, S> {
    /// Creates a new [`TxTestCase::Applied`].
    pub fn applied(message: MessageType<M, S>, post_dispatch_hook: WorkingSetClosure<RT>) -> Self {
        Self::Applied {
            message,
            post_dispatch_hook,
        }
    }

    /// Creates a new [`TxTestCase::Reverted`].
    /// Since the transaction is supposed to revert, there is no need to provide a post_dispatch_hook.
    pub fn reverted(message: MessageType<M, S>, reason: ModuleError) -> Self {
        Self::Reverted { message, reason }
    }

    /// Creates a new [`TxTestCase`] which is skipped.
    pub fn skipped(message: MessageType<M, S>, skipped_reason: SkippedReason) -> Self {
        Self::Skipped {
            message,
            skipped_reason,
        }
    }

    /// Creates a new [`TxTestCase`] which is dropped.
    pub fn dropped(message: MessageType<M, S>) -> Self {
        Self::Dropped(message)
    }

    /// Creates a new [`TxTestCase`] from an expected outcome. Doesn't include a post_dispatch_hook in the successful case.
    /// If the effect is [`None`], the transaction is dropped.
    pub fn from_expected_outcome(
        message: MessageType<M, S>,
        effect: Option<TxEffect<TxReceiptContents>>,
    ) -> Self {
        match effect {
            Some(TxEffect::Successful(_)) => Self::Applied {
                message,
                post_dispatch_hook: Box::new(|_| {}),
            },
            Some(TxEffect::Reverted(reason)) => Self::Reverted { message, reason },
            Some(TxEffect::Skipped(skipped_reason)) => Self::Skipped {
                message,
                skipped_reason,
            },
            None => Self::Dropped(message),
        }
    }
}
