use sov_mock_da::MockDaSpec;
use sov_modules_api::hooks::TxHooks;
use sov_modules_api::{Module, Spec, StateCheckpoint};
use sov_modules_stf_blueprint::Runtime;

use super::messages::MessageType;
use crate::runtime::wrapper::EndSlotClosure;
use crate::runtime::{TxRunner, WorkingSetClosure};

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

/// Defines a test case at the batch level. This can be used to describe a rollup's test. It contains a list of [`TxTestCase`]s.
pub type BatchTestCase<RT, M, S> = Vec<TxTestCase<RT, M, S>>;

impl<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> SlotTestCase<RT, M, S> {
    /// Creates an empty [`SlotTestCase`].
    pub fn empty() -> Self {
        Self {
            batch_test_cases: vec![],
            post_hook: Box::new(|_| {}),
        }
    }

    /// Creates a [`SlotTestCase`] from a list of [`TxTestCase`]s.
    pub fn from_txs(test_cases: Vec<TxTestCase<RT, M, S>>) -> Self {
        Self {
            batch_test_cases: vec![test_cases],
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

/// Defines the expected outcome of a transaction.
pub enum TxExpectedResult {
    /// Expects that the tx was successful
    Applied,
    /// Expects that the tx was reverted
    Reverted,
}
/// Defines the expected outcome of a batch. This is simply a list of [`TxExpectedResult`]s.
pub type BatchExpectedResult = Vec<TxExpectedResult>;
/// Defines the expected outcomes of a slot. This is simply a list of [`BatchExpectedResult`]s.
pub type SlotExpectedResult = Vec<BatchExpectedResult>;

/// Defines the expected outcome of a transaction. If the transaction is successfully applied, one can provide a closure to be executed in the post_dispatch hook.
pub enum TxOutcome<RT: TxHooks> {
    /// Expects that the tx was successful and runs the provided closure in the post_dispatch hook
    Applied(WorkingSetClosure<RT>),
    /// Expects that the tx was reverted
    Reverted,
}

impl<RT: TxHooks> TxOutcome<RT> {
    /// Creates an [`TxOutcome`] that expects the transaction to be successfully applied without any post_dispatch hook closure.
    pub fn applied() -> Self {
        Self::Applied(Box::new(|_| {}))
    }
}

/// Defines a test case at the transaction level. It contains a [`TxOutcome`] which may specify a `post_dispatch_hook` closure and a [`MessageType`].
///
/// ## Example
/// ```rust
/// use sov_modules_api::PrivateKey;
/// use sov_modules_api::transaction::UnsignedTransaction;
/// use sov_modules_api::hooks::TxHooks;
/// use sov_test_utils::runtime::ValueSetter;
/// use sov_test_utils::{TestPrivateKey, TestSpec, TxOutcome, MessageType, TxTestCase};
/// use sov_mock_da::MockDaSpec;
///
/// let priv_key = TestPrivateKey::generate();
/// sov_test_utils::generate_optimistic_runtime!(TestRuntime <= value_setter: ValueSetter<S>);
///
/// // This means to send a transaction that sets the value setter's state to 10 and expects it to be successfully applied
/// TxTestCase {
///     outcome: TxOutcome::Applied::<TestRuntime<TestSpec, MockDaSpec>>(Box::new(|state| {
///         // Check that the state of the rollup has been updated correctly
///     })),
///     message: MessageType::<ValueSetter<TestSpec>, TestSpec>::Plain(sov_value_setter::CallMessage::SetValue(10), priv_key),
/// };
/// ```
pub struct TxTestCase<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> {
    /// The expected outcome of the transaction.
    pub outcome: TxOutcome<RT>,
    /// The message to be sent to the runtime.
    pub message: MessageType<M, S>,
}

impl<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> TxTestCase<RT, M, S> {
    /// Splits a [`TxTestCase`] into a [`TxRunner`] and an optional [`WorkingSetClosure`].
    pub fn split(self) -> (TxRunner<S, M>, Option<WorkingSetClosure<RT>>) {
        let (expected_result, is_post_check): (TxExpectedResult, Option<_>) = match self.outcome {
            TxOutcome::Applied(closure) => (TxExpectedResult::Applied, Option::Some(closure)),
            TxOutcome::Reverted => (TxExpectedResult::Reverted, None),
        };

        (
            TxRunner {
                message: self.message,
                expected_result,
            },
            is_post_check,
        )
    }

    /// Creates a new [`TxTestCase`] with the [`TxOutcome::Applied`] outcome.
    pub fn applied(message: MessageType<M, S>, post_dispatch_hook: WorkingSetClosure<RT>) -> Self {
        Self {
            outcome: TxOutcome::Applied(post_dispatch_hook),
            message,
        }
    }

    /// Creates a new [`TxTestCase`] with the [`TxOutcome::Reverted`] outcome.
    /// Since the transaction is supposed to revert, there is no need to provide a post_dispatch_hook.
    pub fn reverted(message: MessageType<M, S>) -> Self {
        Self {
            outcome: TxOutcome::Reverted,
            message,
        }
    }
}
