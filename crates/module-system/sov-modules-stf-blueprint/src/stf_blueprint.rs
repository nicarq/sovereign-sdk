use std::marker::PhantomData;

use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::{DaSpec, Gas, ProofReceipt, Spec, StateCheckpoint, Storage};
use sov_rollup_interface::stf::StoredEvent;
use sov_state::StorageProof;

use crate::proof_processing::process_proof;
use crate::Runtime;
/// An implementation of the
/// [`StateTransitionFunction`](sov_rollup_interface::stf::StateTransitionFunction)
/// that is specifically designed to work with the module-system.
pub struct StfBlueprint<S: Spec, RT: Runtime<S>, K: KernelSlotHooks<S>> {
    /// The runtime includes all the modules that the rollup supports.
    pub(crate) runtime: RT,
    pub(crate) kernel: K,
    phantom_context: PhantomData<S>,
}

impl<S, RT, K> Default for StfBlueprint<S, RT, K>
where
    S: Spec,
    RT: Runtime<S>,
    K: KernelSlotHooks<S>,
{
    fn default() -> Self {
        Self {
            runtime: RT::default(),
            kernel: K::default(),
            phantom_context: PhantomData,
        }
    }
}

impl<S, RT, K> StfBlueprint<S, RT, K>
where
    S: Spec,
    RT: Runtime<S>,
    K: KernelSlotHooks<S>,
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

    #[allow(clippy::type_complexity)]
    pub(crate) fn process_proof(
        &self,
        blob_hash: [u8; 32],
        sender: <S::Da as DaSpec>::Address,
        gas_price: &<S::Gas as Gas>::Price,
        raw_proof: Vec<u8>,
        checkpoint: StateCheckpoint<S::Storage>,
    ) -> (
        ProofReceipt<
            S::Address,
            S::Da,
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
pub(crate) fn convert_to_runtime_events<S, RT>(
    events: Vec<sov_modules_api::TypedEvent>,
) -> Vec<StoredEvent>
where
    S: Spec,
    RT: Runtime<S>,
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
pub(crate) fn convert_to_runtime_events<S, RT>(
    _events: Vec<sov_modules_api::TypedEvent>,
) -> Vec<StoredEvent>
where
    S: Spec,
    RT: Runtime<S>,
{
    Vec::new() // Return an empty vector
}
