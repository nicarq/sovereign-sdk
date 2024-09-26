use std::marker::PhantomData;

use borsh::BorshDeserialize;
use sov_modules_api::capabilities::{
    AuthorizeSequencerError, GasEnforcer, HasCapabilities, ProofProcessor, SequencerAuthorization,
    SequencerRemuneration, TryReserveGasError,
};
use sov_modules_api::proof_metadata::{ProofType, SerializeProofWithDetails};
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{
    DaSpec, Gas, InvalidProofError, PreExecWorkingSet, ProofOutcome, ProofReceipt,
    ProofReceiptContents, Spec, StateCheckpoint, TxScratchpad, WorkingSet,
};
use sov_state::{Storage, StorageProof};

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
    state: StateCheckpoint<S::Storage>,
) -> (ProcessProofOutput<S, Da>, StateCheckpoint<S::Storage>)
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
                    WorkflowResult::EarlyReturn(out, state) => {
                        tracing::debug!("{LOG_PREFIX}: unable to create pre execution working set");
                        return (out, state);
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
                WorkflowResult::EarlyReturn(out, state) => {
                    tracing::debug!(
                        "{LOG_PREFIX}: unable to reserve gas for the proof verification"
                    );
                    return (out, state);
                }
            };

            let receipt_contents = match proof_with_details.proof {
                ProofType::ZkAggregatedProof(proof) => runtime
                    .proof_processor()
                    .process_aggregated_proof(proof, &sequencer_rollup_address, &mut working_set)
                    .map(|(pub_data, proof)| ProofReceiptContents::AggregateProof(pub_data, proof)),

                ProofType::OptimisticProofAttestation(proof) => runtime
                    .proof_processor()
                    .process_attestation(proof, &sequencer_rollup_address, &mut working_set)
                    .map(|attestation| ProofReceiptContents::Attestation(attestation)),

                ProofType::OptimisticProofChallenge(proof, rollup_height) => runtime
                    .proof_processor()
                    .process_challenge(
                        proof,
                        rollup_height,
                        &sequencer_rollup_address,
                        &mut working_set,
                    )
                    .map(|challenge| ProofReceiptContents::BlockProof(challenge)),
            };

            let (outcome, mut tx_scratchpad, transaction_consumption) = match receipt_contents {
                Ok(receipt_contents) => {
                    let (tx_scratchpad, transaction_consumption, _) = working_set.finalize();
                    (
                        ProofOutcome::Valid(receipt_contents),
                        tx_scratchpad,
                        transaction_consumption,
                    )
                }
                Err(e) if e.is_not_revertable() => {
                    let (tx_scratchpad, transaction_consumption, _) = working_set.finalize();
                    (
                        ProofOutcome::Invalid(e),
                        tx_scratchpad,
                        transaction_consumption,
                    )
                }
                Err(e) => {
                    let (tx_scratchpad, transaction_consumption) = working_set.revert();
                    (
                        ProofOutcome::Invalid(e),
                        tx_scratchpad,
                        transaction_consumption,
                    )
                }
            };

            runtime.gas_enforcer().refund_remaining_gas(
                &sequencer_rollup_address,
                &transaction_consumption.remaining_funds(),
                &mut tx_scratchpad,
            );

            runtime.gas_enforcer().reward_prover(
                &transaction_consumption.base_fee_value(),
                &mut tx_scratchpad,
            );

            let sequencer_reward = transaction_consumption.priority_fee();
            runtime.sequencer_remuneration().reward_sequencer(
                &sequencer_rollup_address,
                sequencer_reward,
                &mut tx_scratchpad,
            );

            (
                ProcessProofOutput {
                    proof_receipt: ProofReceipt {
                        blob_hash,
                        outcome,
                        gas_used: transaction_consumption.base_fee().as_ref().to_vec(),
                        gas_price: gas_price.as_ref().to_vec(),
                    },
                    gas_used: transaction_consumption.base_fee().clone(),
                },
                tx_scratchpad.commit(),
            )
        }
        Err(_) => {
            // We could not deserialize the data from the DA. Penalize the sequencer and return early.
            tracing::debug!("{LOG_PREFIX}: unable to deserialize the aggregated proof");
            workflow.slash_for_bad_serialization(blob_hash, state)
        }
    }
}

#[allow(clippy::type_complexity)]
pub(crate) struct ProcessProofOutput<S: Spec, Da: DaSpec> {
    pub(crate) proof_receipt: ProofReceipt<
        S::Address,
        Da,
        <S::Storage as Storage>::Root,
        StorageProof<<S::Storage as Storage>::Proof>,
    >,

    pub(crate) gas_used: S::Gas,
}

// Decides if the proof processing workflow should continue or return early.
#[allow(clippy::large_enum_variant)]
enum WorkflowResult<Arg, S: Spec, Da: DaSpec> {
    // Proceed with the proof processing.
    Proceed(Arg),
    // Early return from the proof processing.
    EarlyReturn(ProcessProofOutput<S, Da>, StateCheckpoint<S::Storage>),
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
        tx_scratchpad: TxScratchpad<S::Storage>,
    ) -> PreExecWorkingSetResult<S, Da, RT> {
        match self.runtime.sequencer_authorization().authorize_sequencer(
            self.sequencer_da_address,
            gas_price,
            tx_scratchpad,
        ) {
            Ok((allowed_sequencer, pre_exec_working_set)) => {
                WorkflowResult::Proceed((allowed_sequencer.address, pre_exec_working_set))
            }
            Err(AuthorizeSequencerError {
                reason,
                tx_scratchpad,
            }) => WorkflowResult::EarlyReturn(
                ProcessProofOutput {
                    proof_receipt: invalid_proof_receipt::<S, Da>(
                        self.blob_hash,
                        InvalidProofError::PreconditionNotMet(format!(
                            "Failed to authorize sequencer: {}",
                            reason
                        )),
                    ),
                    gas_used: S::Gas::zero(),
                },
                tx_scratchpad.commit(),
            ),
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
        match self.runtime.gas_enforcer().try_reserve_gas(
            &auth_tx,
            sequencer_rollup_address,
            pre_exec_working_set,
        ) {
            Ok(working_set) => WorkflowResult::Proceed(working_set),
            Err(TryReserveGasError {
                reason,
                pre_exec_working_set,
            }) => {
                let reason_str = reason.to_string();
                WorkflowResult::EarlyReturn(
                    ProcessProofOutput {
                        proof_receipt: invalid_proof_receipt::<S, Da>(
                            self.blob_hash,
                            InvalidProofError::PreconditionNotMet(format!(
                                "Failed to reserve gas: {}",
                                reason_str
                            )),
                        ),
                        gas_used: S::Gas::zero(),
                    },
                    self.penalize_sequencer(reason, pre_exec_working_set)
                        .commit(),
                )
            }
        }
    }

    fn slash_for_bad_serialization(
        &self,
        blob_hash: [u8; 32],
        mut state: StateCheckpoint<S::Storage>,
    ) -> (ProcessProofOutput<S, Da>, StateCheckpoint<S::Storage>) {
        self.runtime
            .sequencer_remuneration()
            .slash_sequencer(self.sequencer_da_address, &mut state);

        (
            ProcessProofOutput {
                proof_receipt: invalid_proof_receipt::<S, Da>(
                    blob_hash,
                    InvalidProofError::PreconditionNotMet(
                        "Sequencer slashed for invalid serialization".to_string(),
                    ),
                ),
                gas_used: S::Gas::zero(),
            },
            state,
        )
    }

    fn penalize_sequencer(
        &self,
        reason: impl std::fmt::Display,
        pre_exec_working_set: PreExecWorkingSet<
            S,
            <RT as HasCapabilities<S, Da>>::SequencerStakeMeter,
        >,
    ) -> TxScratchpad<S::Storage> {
        self.runtime.sequencer_authorization().penalize_sequencer(
            self.sequencer_da_address,
            reason,
            pre_exec_working_set,
        )
    }
}

#[allow(clippy::type_complexity)]
fn invalid_proof_receipt<S: Spec, Da: DaSpec>(
    blob_hash: [u8; 32],
    reason: InvalidProofError,
) -> ProofReceipt<
    S::Address,
    Da,
    <S::Storage as Storage>::Root,
    StorageProof<<S::Storage as Storage>::Proof>,
> {
    ProofReceipt {
        blob_hash,
        outcome: ProofOutcome::Invalid(reason),
        gas_used: S::Gas::zero().as_ref().to_vec(),
        gas_price: Vec::new(),
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
