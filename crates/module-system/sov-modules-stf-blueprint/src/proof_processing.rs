use std::marker::PhantomData;

use sov_modules_api::capabilities::{
    GasEnforcer, ProofProcessor, SequencerAuthorization, SequencerRemuneration, TryReserveGasError,
};
use sov_modules_api::proof_metadata::{ProofType, SerializeProofWithDetails};
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{
    BasicGasMeter, DaSpec, Gas, GasArray, GasMeter, GasSpec, InvalidProofError,
    MeteredBorshDeserialize, PreExecWorkingSet, ProofOutcome, ProofReceipt, ProofReceiptContents,
    SlotGasMeter, Spec, StateCheckpoint, StateProvider, TxScratchpad, WorkingSet,
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
    slot_gas_meter: &SlotGasMeter<S>,
    blob_hash: [u8; 32],
    sequencer_da_address: <S::Da as DaSpec>::Address,
    gas_price: &<S::Gas as Gas>::Price,
    raw_proof: Vec<u8>,
    state: StateCheckpoint<S>,
) -> (ProcessProofOutput<S>, StateCheckpoint<S>)
where
    S: Spec,
    RT: Runtime<S>,
{
    let workflow = ProofProcessingWorkflow::new(runtime, blob_hash, &sequencer_da_address);

    // Check if the sequencer is bonded, and create `pre_exec_working_set`.
    let (sequencer_rollup_address, mut pre_exec_working_set) =
        match workflow.authorize_sequencer(slot_gas_meter, gas_price, state.to_tx_scratchpad()) {
            WorkflowResult::Proceed(pre_exec_working_set) => pre_exec_working_set,
            WorkflowResult::EarlyReturn(out, scratchpad) => {
                tracing::debug!("{LOG_PREFIX}: unable to create pre execution working set");

                // If sequencer authorization failed we don't charge any gas.
                return (out, scratchpad.commit());
            }
        };

    // The `pre_exec_working_set` is initialize, so we can start charging gas.
    match SerializeProofWithDetails::<S>::deserialize(
        &mut raw_proof.as_slice(),
        &mut pre_exec_working_set,
    ) {
        Ok(proof_with_details) => {
            // Reserve gas for the proof verification.
            let mut working_set = match workflow.try_reserve_gas(
                slot_gas_meter,
                &sequencer_rollup_address,
                gas_price,
                proof_with_details.details.into(),
                pre_exec_working_set,
            ) {
                WorkflowResult::Proceed(working_set) => working_set,
                WorkflowResult::EarlyReturn(out, scratchpad) => {
                    tracing::debug!(
                        "{LOG_PREFIX}: unable to reserve gas for the proof verification"
                    );

                    tracing::info!(
                        sequencer = %sequencer_da_address,
                        gas_used = %out.gas_used,
                        "The sequencer paid for the transaction.",
                    );

                    let state = workflow
                        .charge_sequencer_and_reward_prover(
                            out.gas_used.value(gas_price),
                            scratchpad,
                        )
                        .commit();

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

            let gas_used = transaction_consumption.base_fee().clone();
            (
                ProcessProofOutput {
                    proof_receipt: ProofReceipt {
                        blob_hash,
                        outcome,
                        gas_used: transaction_consumption.base_fee().as_ref().to_vec(),
                        gas_price: gas_price.as_ref().to_vec(),
                    },
                    gas_used,
                },
                scratchpad.commit(),
            )
        }
        Err(e) => {
            // We could not deserialize the data from the DA. Penalize the sequencer and return early.
            tracing::debug!("{LOG_PREFIX}: unable to deserialize proof {:?}", e);

            let (state, gas_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();
            let gas_used = gas_meter.gas_info().gas_used;

            tracing::info!(
                sequencer = %sequencer_da_address,
                "The sequencer paid for the transaction.",
            );

            let state = workflow
                .charge_sequencer_and_reward_prover(gas_used.value(gas_price), state)
                .commit();

            (
                ProcessProofOutput {
                    proof_receipt: invalid_proof_receipt::<S>(
                        blob_hash,
                        InvalidProofError::PreconditionNotMet(
                            "Sequencer penalized for invalid serialization".to_string(),
                        ),
                    ),
                    gas_used,
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
    EarlyReturn(ProcessProofOutput<S>, TxScratchpad<S, I>),
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
        slot_gas_meter: &SlotGasMeter<S>,
        gas_price: &<S::Gas as Gas>::Price,
        mut tx_scratchpad: TxScratchpad<S, I>,
    ) -> PreExecWorkingSetResult<S, I> {
        let max_tx_check_costs = <S as GasSpec>::max_tx_check_costs();
        let max_tx_check_value = max_tx_check_costs.value(gas_price);

        let sequencer = self
            .runtime
            .sequencer_authorization()
            .authorize_sequencer(self.sequencer_da_address, &mut tx_scratchpad)
            .expect("Blob selection must guarantee that sequencer is registered");

        if sequencer.balance <= max_tx_check_value {
            return WorkflowResult::EarlyReturn(
                ProcessProofOutput {
                    proof_receipt: invalid_proof_receipt::<S>(
                        self.blob_hash,
                        InvalidProofError::PreconditionNotMet(format!(
                            "Sequencer balance insufficient for tx check costs: {}",
                            sequencer.balance
                        )),
                    ),
                    gas_used: S::Gas::zero(),
                },
                tx_scratchpad,
            );
        }

        if slot_gas_meter
            .remaining_slot_gas()
            .dim_is_less_or_eq(&max_tx_check_costs)
        {
            return WorkflowResult::EarlyReturn(
                ProcessProofOutput {
                    proof_receipt: invalid_proof_receipt::<S>(
                        self.blob_hash,
                        InvalidProofError::PreconditionNotMet("Slot run out of gas".to_string()),
                    ),
                    gas_used: S::Gas::zero(),
                },
                tx_scratchpad,
            );
        }

        let pre_exec_gas_meter =
            BasicGasMeter::<S>::new_with_gas(max_tx_check_costs, gas_price.clone());

        let mut pre_exec_working_set = tx_scratchpad.to_pre_exec_working_set(pre_exec_gas_meter);

        // This represents the cost incurred by the sequencer solely for accepting the proof. It includes the cost of:
        // - refund_remaining_gas
        // - reward_sequencer
        // etc
        pre_exec_working_set
            .charge_gas(&<S as GasSpec>::process_tx_pre_exec_checks_gas())
            // It is ok to expect here because `pre_exec_gas_meter` was initialized with `max_tx_check_costs` which is bigger than process_tx_pre_exec_checks_gas.
            .expect("The gas meter should be able to charge the pre-execution checks");

        WorkflowResult::Proceed((sequencer.address, pre_exec_working_set))
    }

    fn try_reserve_gas<I: StateProvider<S>>(
        &self,
        slot_gas_meter: &SlotGasMeter<S>,
        sequencer_rollup_address: &S::Address,
        gas_price: &<S::Gas as Gas>::Price,
        auth_tx: AuthenticatedTransactionData<S>,
        pre_exec_working_set: PreExecWorkingSet<S, I>,
    ) -> WorkflowResult<WorkingSet<S, I>, S, I> {
        let (mut scratchpad, gas_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();
        let gas_info = gas_meter.gas_info();

        if let Err(TryReserveGasError { reason }) =
            self.runtime.gas_enforcer().try_reserve_gas_for_proof(
                &auth_tx,
                gas_price,
                sequencer_rollup_address,
                &mut scratchpad,
            )
        {
            return self.make_early_return(scratchpad, reason, gas_info.gas_used);
        }

        let working_set_gas_meter =
            match auth_tx.gas_meter(gas_price, slot_gas_meter.remaining_slot_gas().clone()) {
                Ok(ws) => ws,
                Err(e) => {
                    return self.make_early_return(
                        scratchpad,
                        format!("Insufficient slot gas {}", e),
                        gas_info.gas_used,
                    );
                }
            };

        let mut working_set =
            WorkingSet::create_working_set(scratchpad, &auth_tx, working_set_gas_meter);

        if let Err(err) = working_set.charge_gas(&gas_info.gas_used) {
            let (scratchpad, _transaction_consumption) = working_set.revert();

            return self.make_early_return(scratchpad, err.to_string(), gas_info.gas_used);
        }

        WorkflowResult::Proceed(working_set)
    }

    fn charge_sequencer_and_reward_prover<I: StateProvider<S>>(
        &self,
        max_auth_cost: u64,
        mut state: TxScratchpad<S, I>,
    ) -> TxScratchpad<S, I> {
        self.runtime
            .gas_enforcer()
            .transfer_funds_from_sequencer_to_prover(
                max_auth_cost,
                self.sequencer_da_address,
                &mut state,
            )
            // This **should** never fail because we initialize gas meter with `max_tx_check_costs` which is lower than the sequencer bond..
            .expect("Sequencer should have enough funds to pay for the penalty");

        state
    }

    fn make_early_return<I: StateProvider<S>>(
        &self,
        tx_scratchpad: TxScratchpad<S, I>,
        reason: String,
        gas_used: S::Gas,
    ) -> WorkflowResult<WorkingSet<S, I>, S, I> {
        WorkflowResult::EarlyReturn(
            ProcessProofOutput {
                proof_receipt: invalid_proof_receipt::<S>(
                    self.blob_hash,
                    InvalidProofError::PreconditionNotMet(reason),
                ),
                gas_used,
            },
            tx_scratchpad,
        )
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
