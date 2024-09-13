use borsh::BorshDeserialize;
use sov_mock_da::MockDaSpec;
use sov_modules_api::{
    ApiStateAccessor, BatchReceipt, BatchSequencerReceipt, DaSpec, Module, ProofReceipt,
    RuntimeEventProcessor, Spec, TransactionReceipt, TxEffect,
};
pub use sov_modules_stf_blueprint::TxReceiptContents;
use sov_state::{Storage, StorageProof};

use super::{BatchType, ProofInput, TransactionType};

type TestAssertion<Context, S> = Box<dyn FnOnce(Context, &mut ApiStateAccessor<S>)>;

/// Context that is passed to [`TransactionTestCase::assert`] to check the outcome of a test.
pub struct TransactionAssertContext<S: Spec, RT: RuntimeEventProcessor> {
    /// The gas used to execute the transaction.
    pub gas_value_used: u64,
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
    ///     GeneratedRuntimeEvent::Bank(
    ///         sov_bank::event::Event::TokenCreated { .. }
    /// ));
    /// ```
    ///
    pub events: Vec<RT::RuntimeEvent>,
    /// The outcome of the transaction.
    pub tx_receipt: TxEffect<TxReceiptContents<S>>,
}

impl<S: Spec, RT: RuntimeEventProcessor> TransactionAssertContext<S, RT> {
    /// Creates a [`TransactionAssertContext`] from the given [`TransactionReceipt`].
    pub fn from_receipt<Da: DaSpec>(
        receipt: TransactionReceipt<TxReceiptContents<S>>,
        gas_value_used: u64,
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
            tx_receipt: receipt.receipt,
            events,
            gas_value_used,
        }
    }
}

/// A closure used to assert the outcome of a [`TransactionTestCase`].
pub type TransactionTestAssert<S, RT> = TestAssertion<TransactionAssertContext<S, RT>, S>;

/// A test case that applies the provided input and asserts the result.
pub struct TransactionTestCase<S: Spec, RT: RuntimeEventProcessor, M: Module> {
    /// Input transaction to execute.
    pub input: TransactionType<M, S>,
    /// Closure used to assert the outcome of the input application
    /// to the rollup state.
    pub assert: TransactionTestAssert<S, RT>,
}

/// Context that is passed to [`BatchTestCase::assert`] to check the outcome of a test.
pub struct BatchAssertContext<S: Spec, Da: DaSpec> {
    /// The DA address of the sender of the batch.
    pub sender_da_address: Da::Address,
    /// The outcome of the batch submission
    ///
    /// This can be [`None`] if the batch was dropped before it was executed,
    /// this can happen if the sender was not a registered sequencer.
    pub batch_receipt:
        Option<BatchReceipt<BatchSequencerReceipt<MockDaSpec>, TxReceiptContents<S>>>,
}

/// A closure used to assert the outcome of a [`BatchTestCase`].
pub type BatchTestAssert<S, Da> = TestAssertion<BatchAssertContext<S, Da>, S>;

/// A test case that applies the provided batch input and asserts the result.
pub struct BatchTestCase<S: Spec, Da: DaSpec, M: Module> {
    /// Input to execute as part of the batch.
    pub input: BatchType<M, S>,
    /// Closure used to assert the outcome of applying the batch to the rollup.
    pub assert: BatchTestAssert<S, Da>,
}

/// Context that is passed to [`ProofTestCase::assert`] to check the outcome of a test.
pub struct ProofAssertContext<S: Spec, Da: DaSpec> {
    /// The outcome of the proof submission.
    ///
    /// This can be [`None`] if the proof was dropped before it was executed,
    /// this can happen if the proof was malformed by the prover. Generally this should always be
    /// present.
    #[allow(clippy::type_complexity)]
    pub proof_receipt: Option<
        ProofReceipt<
            <S as Spec>::Address,
            Da,
            <<S as Spec>::Storage as Storage>::Root,
            StorageProof<<S::Storage as Storage>::Proof>,
        >,
    >,

    /// The gas used to verify the proof.
    pub gas_value_used: u64,
}

/// A closure used to assert the outcome of a [`ProofTestCase`].
pub type ProofTestAssert<S, Da> = TestAssertion<ProofAssertContext<S, Da>, S>;

/// A test case that applies the provided proof input and asserts the result.
pub struct ProofTestCase<S: Spec, Da: DaSpec> {
    /// Input for the test case.
    pub input: ProofInput,
    /// Assertion for the test case.
    pub assert: ProofTestAssert<S, Da>,
}
