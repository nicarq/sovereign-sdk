use std::marker::PhantomData;

use borsh::BorshDeserialize;
use sov_modules_api::capabilities::{
    AuthorizeSequencerError, GasEnforcer, ProofProcessor, SequencerAuthorization,
    SequencerRemuneration, TryReserveGasError,
};
use sov_modules_api::proof_metadata::{ProofType, SerializeProofWithDetails};
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{
    BasicGasMeter, DaSpec, Gas, GasMeter, InvalidProofError, PreExecWorkingSet, ProofOutcome,
    ProofReceipt, ProofReceiptContents, Spec, StateCheckpoint, StateProvider, TxScratchpad,
    WorkingSet,
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
pub(crate) fn process_proof<S, RT>(
    runtime: &RT,
    blob_hash: [u8; 32],
    sequencer_da_address: <S::Da as DaSpec>::Address,
    gas_price: &<S::Gas as Gas>::Price,
    raw_proof: Vec<u8>,
    state: StateCheckpoint<S::Storage>,
) -> (ProcessProofOutput<S>, StateCheckpoint<S::Storage>)
where
    S: Spec,
    RT: Runtime<S>,
{
    let workflow = ProofProcessingWorkflow::new(runtime, blob_hash, &sequencer_da_address);

    // We're currently penalizing the sequencer too much, but this is acceptable.
    // Once we measure the cost of deserialization, we can provide a more accurate value.
    let max_pre_exec_check_gas = runtime.gas_enforcer().max_tx_check_costs();
    let max_auth_cost = max_pre_exec_check_gas.value(gas_price);

    // Check if the sequencer is bonded, and create `pre_exec_working_set`.
    let (sequencer_rollup_address, pre_exec_working_set) =
        match workflow.authorize_sequencer(gas_price, max_auth_cost, state.to_tx_scratchpad()) {
            WorkflowResult::Proceed(pre_exec_working_set) => pre_exec_working_set,
            WorkflowResult::EarlyReturn(out, state) => {
                tracing::debug!("{LOG_PREFIX}: unable to create pre execution working set");
                return (out, state);
            }
        };

    match SerializeProofWithDetails::<S>::try_from_slice(&raw_proof) {
        Ok(proof_with_details) => {
            // Reserve gas for the proof verification.
            let mut working_set = match workflow.try_reserve_gas(
                &sequencer_rollup_address,
                gas_price,
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
                    .map(ProofReceiptContents::Attestation),

                ProofType::OptimisticProofChallenge(proof, rollup_height) => runtime
                    .proof_processor()
                    .process_challenge(
                        proof,
                        rollup_height,
                        &sequencer_rollup_address,
                        &mut working_set,
                    )
                    .map(ProofReceiptContents::BlockProof),
            };

            let (outcome, mut scratchpad, transaction_consumption) = match receipt_contents {
                Ok(receipt_contents) => {
                    let (scratchpad, transaction_consumption, _) = working_set.finalize();
                    (
                        ProofOutcome::Valid(receipt_contents),
                        scratchpad,
                        transaction_consumption,
                    )
                }
                Err(e) if e.is_not_revertable() => {
                    let (scratchpad, transaction_consumption, _) = working_set.finalize();
                    (
                        ProofOutcome::Invalid(e),
                        scratchpad,
                        transaction_consumption,
                    )
                }
                Err(e) => {
                    let (scratchpad, transaction_consumption) = working_set.revert();
                    (
                        ProofOutcome::Invalid(e),
                        scratchpad,
                        transaction_consumption,
                    )
                }
            };

            runtime.gas_enforcer().refund_remaining_gas(
                &sequencer_rollup_address,
                &transaction_consumption.remaining_funds(),
                &mut scratchpad,
            );

            runtime
                .gas_enforcer()
                .reward_prover(&transaction_consumption.base_fee_value(), &mut scratchpad);

            let sequencer_reward = transaction_consumption.priority_fee();
            runtime.sequencer_remuneration().reward_sequencer(
                &sequencer_da_address,
                sequencer_reward,
                &mut scratchpad,
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
                scratchpad.commit(),
            )
        }
        Err(_) => {
            // We could not deserialize the data from the DA. Penalize the sequencer and return early.
            tracing::debug!("{LOG_PREFIX}: unable to deserialize proof");

            let (state, _) = pre_exec_working_set.to_scratchpad_and_gas_meter();
            let state = workflow
                .charge_sequencer_and_reward_prover(
                    "Unable to deserialize proof",
                    max_auth_cost,
                    state,
                )
                .commit();

            (
                ProcessProofOutput {
                    proof_receipt: invalid_proof_receipt::<S>(
                        blob_hash,
                        InvalidProofError::PreconditionNotMet(
                            "Sequencer penalized for invalid serialization".to_string(),
                        ),
                    ),
                    gas_used: max_pre_exec_check_gas,
                },
                state,
            )
        }
    }
}

#[allow(clippy::type_complexity)]
pub(crate) struct ProcessProofOutput<S: Spec> {
    pub(crate) proof_receipt: ProofReceipt<
        S::Address,
        <S as Spec>::Da,
        <S::Storage as Storage>::Root,
        StorageProof<<S::Storage as Storage>::Proof>,
    >,

    pub(crate) gas_used: S::Gas,
}

// Decides if the proof processing workflow should continue or return early.
#[allow(clippy::large_enum_variant)]
enum WorkflowResult<Arg, S: Spec, I: StateProvider<S>> {
    // Proceed with the proof processing.
    Proceed(Arg),
    // Early return from the proof processing.
    EarlyReturn(ProcessProofOutput<S>, I),
}

struct ProofProcessingWorkflow<'a, S: Spec, RT: Runtime<S>> {
    runtime: &'a RT,
    blob_hash: [u8; 32],
    sequencer_da_address: &'a <<S as Spec>::Da as DaSpec>::Address,
    _phantom: PhantomData<S>,
}

impl<'a, S, RT> ProofProcessingWorkflow<'a, S, RT>
where
    S: Spec,
    RT: Runtime<S>,
{
    fn new(
        runtime: &'a RT,
        blob_hash: [u8; 32],
        sequencer_da_address: &'a <<S as Spec>::Da as DaSpec>::Address,
    ) -> Self {
        Self {
            runtime,
            blob_hash,
            sequencer_da_address,
            _phantom: PhantomData,
        }
    }

    fn authorize_sequencer<I: StateProvider<S>>(
        &self,
        gas_price: &<S::Gas as Gas>::Price,
        max_auth_cost: u64,
        mut tx_scratchpad: TxScratchpad<S, I>,
    ) -> PreExecWorkingSetResult<S, I> {
        match self.runtime.sequencer_authorization().authorize_sequencer(
            self.sequencer_da_address,
            max_auth_cost,
            &mut tx_scratchpad,
        ) {
            Ok(allowed_sequencer) => {
                let gas_meter =
                    BasicGasMeter::<S::Gas>::new(allowed_sequencer.balance, gas_price.clone());
                let pre_exec_working_set = tx_scratchpad.to_pre_exec_working_set(gas_meter);

                WorkflowResult::Proceed((allowed_sequencer.address, pre_exec_working_set))
            }
            Err(AuthorizeSequencerError { reason }) => WorkflowResult::EarlyReturn(
                ProcessProofOutput {
                    proof_receipt: invalid_proof_receipt::<S>(
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

    fn try_reserve_gas<I: StateProvider<S>>(
        &self,
        sequencer_rollup_address: &S::Address,
        gas_price: &<S::Gas as Gas>::Price,
        auth_tx: AuthenticatedTransactionData<S>,
        pre_exec_working_set: PreExecWorkingSet<S, I>,
    ) -> WorkflowResult<WorkingSet<S, I>, S, I> {
        let (mut scratchpad, gas_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();

        let gas_info = gas_meter.gas_info();
        let auth_cost = gas_info.gas_used.value(gas_price);
        if let Err(TryReserveGasError { reason }) =
            self.runtime.gas_enforcer().try_reserve_gas_for_proof(
                &auth_tx,
                gas_price,
                sequencer_rollup_address,
                &mut scratchpad,
            )
        {
            return WorkflowResult::EarlyReturn(
                ProcessProofOutput {
                    proof_receipt: invalid_proof_receipt::<S>(
                        self.blob_hash,
                        InvalidProofError::PreconditionNotMet(format!(
                            "Failed to reserve gas: {}",
                            reason
                        )),
                    ),
                    gas_used: gas_info.gas_used,
                },
                self.charge_sequencer_and_reward_prover(reason, auth_cost, scratchpad)
                    .commit(),
            );
        }

        let mut working_set =
            WorkingSet::create_working_set(scratchpad, &gas_info.gas_price, &auth_tx);

        if let Err(err) = working_set.charge_gas(&gas_info.gas_used) {
            let (scratchpad, _transaction_consumption) = working_set.revert();

            return WorkflowResult::EarlyReturn(
                ProcessProofOutput {
                    proof_receipt: invalid_proof_receipt::<S>(
                        self.blob_hash,
                        InvalidProofError::PreconditionNotMet(format!(
                            "Failed to reserve gas: {}",
                            err
                        )),
                    ),
                    gas_used: gas_info.gas_used,
                },
                self.charge_sequencer_and_reward_prover(err, auth_cost, scratchpad)
                    .commit(),
            );
        }

        WorkflowResult::Proceed(working_set)
    }

    fn charge_sequencer_and_reward_prover<I: StateProvider<S>>(
        &self,
        reason: impl std::fmt::Display,
        max_auth_cost: u64,
        mut state: TxScratchpad<S, I>,
    ) -> TxScratchpad<S, I> {
        tracing::info!(
            sequencer = %self.sequencer_da_address,
            reason = %reason,
            "The sequencer paid for the transaction.",
        );

        self.runtime
            .gas_enforcer()
            .transfer_funds_from_sequencer_to_prover(
                max_auth_cost,
                self.sequencer_da_address,
                &mut state,
            )
            // We ensured this before we started processing the proof.
            .expect("Sequencer should have enough funds to pay for the penalty");

        state
    }
}

#[allow(clippy::type_complexity)]
fn invalid_proof_receipt<S: Spec>(
    blob_hash: [u8; 32],
    reason: InvalidProofError,
) -> ProofReceipt<
    S::Address,
    <S as Spec>::Da,
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

type PreExecWorkingSetResult<S, I> =
    WorkflowResult<(<S as Spec>::Address, PreExecWorkingSet<S, I>), S, I>;
