use std::marker::PhantomData;

use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::{
    BatchSequencerOutcome, BatchWithId, DaSpec, ExecutionContext, Gas, ProofReceipt, Spec,
    StateCheckpoint, Storage,
};
use sov_rollup_interface::stf::StoredEvent;
use sov_state::StorageProof;
use tracing::{debug, info};

use crate::batch_processing::{apply_batch, get_gas_used, BatchReceipt};
use crate::proof_processing::process_proof;
use crate::Runtime;
/// An implementation of the
/// [`StateTransitionFunction`](sov_rollup_interface::stf::StateTransitionFunction)
/// that is specifically designed to work with the module-system.
pub struct StfBlueprint<S: Spec, Da: DaSpec, RT: Runtime<S, Da>, K: KernelSlotHooks<S, Da>> {
    /// The runtime includes all the modules that the rollup supports.
    pub(crate) runtime: RT,
    pub(crate) kernel: K,
    phantom_context: PhantomData<S>,
    phantom_da: PhantomData<Da>,
}

impl<S, Da, RT, K> Default for StfBlueprint<S, Da, RT, K>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
    K: KernelSlotHooks<S, Da>,
{
    fn default() -> Self {
        Self {
            runtime: RT::default(),
            kernel: K::default(),
            phantom_context: PhantomData,
            phantom_da: PhantomData,
        }
    }
}

impl<S, Da, RT, K> StfBlueprint<S, Da, RT, K>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
    K: KernelSlotHooks<S, Da>,
{
    /// [`StfBlueprint`] constructor with the default [`Runtime`] value. Same as
    /// [`Default::default`].
    pub fn new() -> Self {
        Self::default()
    }

    /// [`StfBlueprint`] constructor with a custom [`Runtime`] value.
    pub fn with_runtime(runtime: RT) -> Self {
        Self {
            runtime,
            ..Default::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_batch(
        &self,
        batch: BatchWithId,
        checkpoint: StateCheckpoint<S::Storage>,
        blob_idx: usize,
        sequencer_da_address: Da::Address,
        gas_price: &<S::Gas as Gas>::Price,
        visible_height: u64,
        is_registered_sequencer: bool,
        execution_context: ExecutionContext,
    ) -> (StateCheckpoint<S::Storage>, BatchReceipt<S, Da>, S::Gas) {
        let (batch_receipt, mut next_checkpoint, gas_used) = apply_batch::<_, _, _, K>(
            &self.runtime,
            checkpoint,
            batch,
            sequencer_da_address,
            gas_price,
            visible_height,
            is_registered_sequencer,
            execution_context,
        );

        let batch_sequencer_receipt = &batch_receipt.inner;

        info!(
            blob_idx,
            blob_hash = hex::encode(batch_receipt.batch_hash),
            sequencer_da_address = %batch_sequencer_receipt.da_address,
            num_txs = batch_receipt.tx_receipts.len(),
            sequencer_outcome = ?batch_receipt.inner,
            ?gas_used,
            "Applied blob and got the sequencer outcome"
        );

        self.runtime
            .end_batch_hook(batch_sequencer_receipt, &mut next_checkpoint);

        info!(sequencer_da_address =
            ?batch_sequencer_receipt.da_address, ?batch_sequencer_receipt.outcome, "BatchSequencerOutcome ");

        if let BatchSequencerOutcome::Slashed(reason) = &batch_sequencer_receipt.outcome {
            info!(sequencer_da_address =
                %batch_sequencer_receipt.da_address, ?reason, "Slashing sequencer");
        }

        for (i, tx_receipt) in batch_receipt.tx_receipts.iter().enumerate() {
            debug!(
                tx_idx = i,
                tx_hash = hex::encode(tx_receipt.tx_hash),
                receipt = ?tx_receipt.receipt,
                gas_used = ?get_gas_used(tx_receipt),
                "Tx receipt"
            );
        }
        (next_checkpoint, batch_receipt, gas_used)
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn process_proof(
        &self,
        blob_hash: [u8; 32],
        sender: Da::Address,
        gas_price: &<S::Gas as Gas>::Price,
        raw_proof: Vec<u8>,
        checkpoint: StateCheckpoint<S::Storage>,
    ) -> (
        ProofReceipt<
            S::Address,
            Da,
            <S::Storage as Storage>::Root,
            StorageProof<<S::Storage as Storage>::Proof>,
        >,
        StateCheckpoint<S::Storage>,
        S::Gas,
    ) {
        let (res, state) = process_proof(
            &self.runtime,
            blob_hash,
            sender,
            gas_price,
            raw_proof,
            checkpoint,
        );

        (res.proof_receipt, state, res.gas_used)
    }
}

#[cfg(feature = "native")]
pub(crate) fn convert_to_runtime_events<S, RT, Da>(
    events: Vec<sov_modules_api::TypedEvent>,
) -> Vec<StoredEvent>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    events
        .into_iter()
        .map(|typed_event| {
            // This seems to be needed because doing `&typed_event.event_key().to_vec()`
            // directly as the first function param to Event::new() is running into a linter bug
            // where it thinks that the to_vec is not necessary.
            // (probably due to the borrow and move in the same statement)
            // https://github.com/rust-lang/rust-clippy/issues/12098
            let key = typed_event.event_key().to_vec();
            StoredEvent::new(
                &key,
                &borsh::to_vec(
                    &<RT as sov_modules_api::RuntimeEventProcessor>::convert_to_runtime_event(
                        typed_event,
                    )
                    .expect("Unknown event type"),
                )
                .expect("unable to serialize event"),
            )
        })
        .collect()
}

#[cfg(not(feature = "native"))]
pub(crate) fn convert_to_runtime_events<S, RT, Da>(
    _events: Vec<sov_modules_api::TypedEvent>,
) -> Vec<StoredEvent>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    Vec::new() // Return an empty vector
}
