use std::marker::PhantomData;

use borsh::BorshDeserialize;
use sov_modules_api::capabilities::{
    AuthorizeSequencerError, GasEnforcer, HasCapabilities, ProofProcessor, SequencerAuthorization,
    SequencerRemuneration, TryReserveGasError,
};
use sov_modules_api::proof_metadata::SerializeProofWithDetails;
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{
    DaSpec, Gas, PreExecWorkingSet, ProofOutcome, ProofReceipt, Spec, StateCheckpoint,
    TxScratchpad, WorkingSet,
};
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;
use sov_state::Storage;

use crate::Runtime;

const LOG_PREFIX: &str = "Returning early from the proof processing workflow";

// Proof processing workflow:
// 1. Check if the sequencer is bonded.
// 2. Check if the sequencer is registered.
// 3. Verify the proof via the `ProofProcessor` capability.
// 4. Return the proof receipt.
// If any of the steps fail, the proof processing workflow is aborted and returns a `ProofReceipt` with a `ProofOutcome::Invalid`` outcome.
#[allow(clippy::type_complexity)]
pub(crate) fn process_proof<S, Da, RT>(
    runtime: &RT,
    blob_hash: [u8; 32],
    sequencer_da_address: Da::Address,
    gas_price: &<S::Gas as Gas>::Price,
    raw_proof: Vec<u8>,
    state: StateCheckpoint<S>,
) -> ProcessProofOutput<S, Da>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    let workflow = ProofProcessingWorkflow::new(runtime, blob_hash, &sequencer_da_address);

    match SerializeProofWithDetails::<S>::try_from_slice(&raw_proof) {
        Ok(proof_with_details) => {
            // Check if the sequencer is bonded, and create `pre_exec_working_set`.
            let (sequencer_rollup_address, pre_exec_working_set) =
                match workflow.authorize_sequencer(gas_price, state.to_tx_scratchpad()) {
                    WorkflowResult::Proceed(pre_exec_working_set) => pre_exec_working_set,
                    WorkflowResult::EarlyReturn(out) => {
                        tracing::debug!("{LOG_PREFIX}: unable to create pre execution working set");
                        return out;
                    }
                };

            // Reserve gas for the proof verification. The sequencer pays for the verification.
            // If the sequencer does not have enough funds, then penalize it and return early.
            let mut working_set = match workflow.try_reserve_gas(
                &sequencer_rollup_address,
                proof_with_details.details.into(),
                pre_exec_working_set,
            ) {
                WorkflowResult::Proceed(working_set) => working_set,
                WorkflowResult::EarlyReturn(out) => {
                    tracing::debug!(
                        "{LOG_PREFIX}: unable to reserve gas for the proof verification"
                    );
                    return out;
                }
            };

            let outcome = runtime.capabilities().process_proof(
                &proof_with_details.proof,
                &sequencer_rollup_address,
                &mut working_set,
            );
            // TODO:#815 here we will verify the proof via the `ProofProcessor` capability.
            let (tx_scratchpad, _transaction_consumption, _events) = working_set.finalize();

            // TODO:#815 here we will return `ProofOutcome::Valid` once we plug in the `ProofProcessor` capability and handle the `_transaction_consumption`.
            ProcessProofOutput {
                proof_receipt: ProofReceipt {
                    raw_proof: proof_with_details.proof,
                    blob_hash,
                    outcome,
                    extra_data: (),
                },
                checkpoint: tx_scratchpad.commit(),
            }
        }
        Err(_) => {
            // We could not deserialize the data from the DA. Penalize the sequencer and return early.
            tracing::debug!("{LOG_PREFIX}: unable to deserialize the aggregated proof");
            workflow.slash_for_bad_serialization(blob_hash, state)
        }
    }
}

pub(crate) struct ProcessProofOutput<S: Spec, Da: DaSpec> {
    pub(crate) proof_receipt: ProofReceipt<S::Address, Da, <S::Storage as Storage>::Root, ()>,
    pub(crate) checkpoint: StateCheckpoint<S>,
}

// Decides if the proof processing workflow should continue or return early.
#[allow(clippy::large_enum_variant)]
enum WorkflowResult<Arg, S: Spec, Da: DaSpec> {
    // Proceed with the proof processing.
    Proceed(Arg),
    // Early return from the proof processing.
    EarlyReturn(ProcessProofOutput<S, Da>),
}

struct ProofProcessingWorkflow<'a, S: Spec, Da: DaSpec, RT: Runtime<S, Da>> {
    runtime: &'a RT,
    blob_hash: [u8; 32],
    sequencer_da_address: &'a Da::Address,
    _phantom: PhantomData<S>,
}

impl<'a, S, Da, RT> ProofProcessingWorkflow<'a, S, Da, RT>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    fn new(runtime: &'a RT, blob_hash: [u8; 32], sequencer_da_address: &'a Da::Address) -> Self {
        Self {
            runtime,
            blob_hash,
            sequencer_da_address,
            _phantom: PhantomData,
        }
    }

    fn authorize_sequencer(
        &self,
        gas_price: &<S::Gas as Gas>::Price,
        tx_scratchpad: TxScratchpad<S>,
    ) -> PreExecWorkingSetResult<S, Da, RT> {
        match self.runtime.capabilities().authorize_sequencer(
            self.sequencer_da_address,
            gas_price,
            tx_scratchpad,
        ) {
            Ok((allowed_sequencer, pre_exec_working_set)) => {
                WorkflowResult::Proceed((allowed_sequencer.address, pre_exec_working_set))
            }
            Err(AuthorizeSequencerError {
                reason: _,
                tx_scratchpad,
            }) => WorkflowResult::EarlyReturn(ProcessProofOutput {
                checkpoint: tx_scratchpad.commit(),
                proof_receipt: invalid_proof_receipt::<S, Da>(self.blob_hash),
            }),
        }
    }

    fn try_reserve_gas(
        &self,
        sequencer_rollup_address: &S::Address,
        auth_tx: AuthenticatedTransactionData<S>,
        pre_exec_working_set: PreExecWorkingSet<
            S,
            <RT as HasCapabilities<S, Da>>::SequencerStakeMeter,
        >,
    ) -> WorkflowResult<WorkingSet<S>, S, Da> {
        match self.runtime.capabilities().try_reserve_gas(
            &auth_tx,
            sequencer_rollup_address,
            pre_exec_working_set,
        ) {
            Ok(working_set) => WorkflowResult::Proceed(working_set),
            Err(TryReserveGasError {
                reason,
                pre_exec_working_set,
            }) => WorkflowResult::EarlyReturn(ProcessProofOutput {
                checkpoint: self
                    .penalize_sequencer(reason, pre_exec_working_set)
                    .commit(),
                proof_receipt: invalid_proof_receipt::<S, Da>(self.blob_hash),
            }),
        }
    }

    fn slash_for_bad_serialization(
        &self,
        blob_hash: [u8; 32],
        mut state: StateCheckpoint<S>,
    ) -> ProcessProofOutput<S, Da> {
        self.runtime
            .capabilities()
            .slash_sequencer(self.sequencer_da_address, &mut state);

        ProcessProofOutput {
            checkpoint: state,
            proof_receipt: invalid_proof_receipt::<S, Da>(blob_hash),
        }
    }

    fn penalize_sequencer(
        &self,
        reason: impl std::fmt::Display,
        pre_exec_working_set: PreExecWorkingSet<
            S,
            <RT as HasCapabilities<S, Da>>::SequencerStakeMeter,
        >,
    ) -> TxScratchpad<S> {
        self.runtime.capabilities().penalize_sequencer(
            self.sequencer_da_address,
            reason,
            pre_exec_working_set,
        )
    }
}

fn invalid_proof_receipt<S: Spec, Da: DaSpec>(
    blob_hash: [u8; 32],
) -> ProofReceipt<S::Address, Da, <S::Storage as Storage>::Root, ()> {
    // TODO:#815 `raw_proof` will be moved to `ProofOutcome::Valid when we plug `ProofProcessor`` capability.
    ProofReceipt {
        raw_proof: SerializedAggregatedProof {
            raw_aggregated_proof: Default::default(),
        },
        blob_hash,
        outcome: ProofOutcome::Invalid,
        extra_data: (),
    }
}

type PreExecWorkingSetResult<S, Da, RT> = WorkflowResult<
    (
        <S as Spec>::Address,
        PreExecWorkingSet<S, <RT as HasCapabilities<S, Da>>::SequencerStakeMeter>,
    ),
    S,
    Da,
>;
