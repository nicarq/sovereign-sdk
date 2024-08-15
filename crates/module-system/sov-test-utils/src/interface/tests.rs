use borsh::BorshDeserialize;
use sov_mock_da::MockDaSpec;
use sov_modules_api::{
    ApiStateAccessor, BatchReceipt, BatchSequencerReceipt, DaSpec, Module, RuntimeEventProcessor,
    Spec, TransactionReceipt, TxEffect,
};
use sov_modules_stf_blueprint::TxReceiptContents;

use super::{BatchType, TransactionType};

/// Context that is passed to [`TransactionTestCase::assert`] to check the outcome of a test.
pub struct TransactionAssertContext<RT: RuntimeEventProcessor> {
    /// The gas used to execute the transaction.
    pub gas_used: u64,
    /// The events raised by the transaction.
    ///
    /// The RuntimeEvent can be checked for specific module events, using the `sov_bank` module
    /// as an example below.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let context = TransactionAssertContext { .. };
    /// let runtime_event = context.events[0];
    /// matches!(
    ///     &runtime_event,
    ///     GeneratedRuntimeEvent::bank(
    ///         sov_bank::event::Event::TokenCreated { .. }
    /// ));
    /// ```
    ///
    pub events: Vec<RT::RuntimeEvent>,
    /// The outcome of the transaction.
    pub outcome: TxEffect<TxReceiptContents>,
}

impl<RT: RuntimeEventProcessor> TransactionAssertContext<RT> {
    /// Creates a [`TransactionAssertContext`] from the given [`TransactionReceipt`].
    pub fn from_receipt<S: Spec, Da: DaSpec>(
        receipt: TransactionReceipt<TxReceiptContents>,
        gas_used: u64,
    ) -> Self {
        let events = receipt
            .events
            .into_iter()
            .map(|stored_event| {
                <RT as RuntimeEventProcessor>::RuntimeEvent::deserialize(
                    &mut stored_event.value().inner().as_slice(),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        TransactionAssertContext {
            outcome: receipt.receipt,
            events,
            gas_used,
        }
    }
}

/// A closure used to assert the outcome of a [`TransactionTestCase`].
pub type TransactionTestAssert<S, RT> =
    dyn FnOnce(TransactionAssertContext<RT>, &mut ApiStateAccessor<S>);

/// A test case that applies the provided input and asserts the result.
pub struct TransactionTestCase<S: Spec, RT: RuntimeEventProcessor, M: Module> {
    /// Input transaction to execute.
    pub input: TransactionType<M, S>,
    /// Closure used to assert the outcome of the input application
    /// to the rollup state.
    pub assert: Box<TransactionTestAssert<S, RT>>,
}

/// Context that is passed to [`BatchTestCase::assert`] to check the outcome of a test.
pub struct BatchAssertContext<Da: DaSpec> {
    /// The DA address of the sender of the batch.
    pub sender_da_address: Da::Address,
    /// The outcome of the batch submission
    ///
    /// This can be [`None`] if the batch was dropped before it was executed,
    /// this can happen if the sender was not a registered sequencer.
    pub outcome: Option<BatchReceipt<BatchSequencerReceipt<MockDaSpec>, TxReceiptContents>>,
}

/// A closure used to assert the outcome of a [`BatchTestCase`].
pub type BatchTestAssert<S, Da> = dyn FnOnce(BatchAssertContext<Da>, &mut ApiStateAccessor<S>);

/// A test case that applies the provided batch input and asserts the result.
pub struct BatchTestCase<S: Spec, Da: DaSpec, M: Module> {
    /// Input to execute as part of the batch.
    pub input: BatchType<M, S>,
    /// Optionally specify the DA address of the sequencer of the batch.
    ///
    /// If this is not provided the default sequencer address in the `TestRunner` will be used.
    pub override_sequencer: Option<Da::Address>,
    /// Closure used to assert the outcome of applying the batch to the rollup.
    pub assert: Box<BatchTestAssert<S, Da>>,
}
