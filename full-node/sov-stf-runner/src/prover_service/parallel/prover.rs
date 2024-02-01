use std::collections::hash_map::Entry;
use std::ops::Deref;
use std::sync::{Arc, RwLock};

use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_rollup_interface::da::{BlockHeaderTrait, DaSpec, DaVerifier};
use sov_rollup_interface::services::da::DaService;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::zk::{Proof, StateTransition, StateTransitionData, ZkvmHost};

use super::state::{ProverState, ProverStatus};
use super::{ProverServiceError, Verifier};
use crate::prover_service::aggregated::{
    AggregatedProofData, AggregatedProofDataInfo, AggregatedProofPublicInput, BlockProof,
};
use crate::verifier::StateTransitionVerifier;
use crate::{
    ProofAggregationStatus, ProofProcessingStatus, RollupProverConfig, StateTransitionInfo,
    WitnessSubmissionStatus,
};

// A prover that generates proofs in parallel using a thread pool. If the pool is saturated,
// the prover will reject new jobs.
pub(crate) struct Prover<StateRoot, Witness, Da: DaService> {
    prover_state: Arc<RwLock<ProverState<StateRoot, Witness, Da::Spec>>>,
    num_threads: usize,
    pool: rayon::ThreadPool,
}

impl<StateRoot, Witness, Da> Prover<StateRoot, Witness, Da>
where
    Da: DaService,
    StateRoot: Serialize + DeserializeOwned + Clone + AsRef<[u8]> + Send + Sync + 'static,
    Witness: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    pub(crate) fn new(num_threads: usize) -> Self {
        Self {
            num_threads,
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(num_threads)
                .build()
                .unwrap(),

            prover_state: Arc::new(RwLock::new(ProverState {
                prover_status: Default::default(),
                pending_tasks_count: Default::default(),
            })),
        }
    }

    pub(crate) fn submit_state_transition_info(
        &self,
        state_transition_info: StateTransitionInfo<StateRoot, Witness, Da::Spec>,
    ) -> WitnessSubmissionStatus {
        let header_hash = state_transition_info.da_block_header().hash();
        let data = ProverStatus::WitnessSubmitted(state_transition_info);

        let mut prover_state = self.prover_state.write().expect("Lock was poisoned");
        let entry = prover_state.prover_status.entry(header_hash.clone());

        match entry {
            Entry::Occupied(_) => WitnessSubmissionStatus::WitnessExist,
            Entry::Vacant(v) => {
                v.insert(data);
                WitnessSubmissionStatus::SubmittedForProving
            }
        }
    }

    pub(crate) fn start_proving<Vm, V>(
        &self,
        block_header_hash: <Da::Spec as DaSpec>::SlotHash,
        config: Arc<RollupProverConfig>,
        mut vm: Vm,
        zk_storage: V::PreState,
        verifier: Arc<Verifier<Da, Vm, V>>,
    ) -> Result<ProofProcessingStatus, ProverServiceError>
    where
        Vm: ZkvmHost + 'static,
        V: StateTransitionFunction<Vm::Guest, Da::Spec> + Send + Sync + 'static,
        V::PreState: Send + Sync + 'static,
    {
        let prover_state_clone = self.prover_state.clone();
        let mut prover_state = self.prover_state.write().expect("Lock was poisoned");

        let prover_status = prover_state
            .remove(&block_header_hash)
            .ok_or_else(|| anyhow::anyhow!("Missing witness for block: {:?}", block_header_hash))?;

        match prover_status {
            ProverStatus::WitnessSubmitted(state_transition_info) => {
                let start_prover = prover_state.inc_task_count_if_not_busy(self.num_threads);

                // Initiate a new proving job only if the prover is not busy.
                if start_prover {
                    prover_state.set_to_proving(block_header_hash.clone());
                    vm.add_hint(&state_transition_info.data);

                    self.pool.spawn(move || {
                        tracing::info_span!("guest_execution").in_scope(|| {
                            let proof = make_proof::<_, _, Da>(
                                vm,
                                config,
                                zk_storage,
                                &verifier.stf_verifier,
                            );

                            let mut prover_state =
                                prover_state_clone.write().expect("Lock was poisoned");

                            let StateTransitionData {
                                initial_state_root,
                                final_state_root,
                                da_block_header,
                                inclusion_proof,
                                completeness_proof,
                                blobs,
                                ..
                            } = state_transition_info.data;

                            let validity_condition = verifier
                                .da_verifier
                                .verify_relevant_tx_list(
                                    &da_block_header,
                                    &blobs,
                                    inclusion_proof,
                                    completeness_proof,
                                )
                                .expect("Invalid validity condition");

                            let block_proof = proof.map(|p| BlockProof {
                                _proof: p,
                                st: StateTransition {
                                    initial_state_root,
                                    final_state_root,
                                    slot_hash: block_header_hash.clone(),
                                    validity_condition,
                                },
                                slot_number: state_transition_info.slot_number,
                            });

                            assert_eq!(block_header_hash, da_block_header.hash());

                            prover_state.set_to_proved(block_header_hash, block_proof);
                            prover_state.dec_task_count();
                        })
                    });

                    Ok(ProofProcessingStatus::ProvingInProgress)
                } else {
                    Ok(ProofProcessingStatus::Busy)
                }
            }
            ProverStatus::ProvingInProgress => Err(anyhow::anyhow!(
                "Proof generation for {:?} still in progress",
                block_header_hash
            )
            .into()),
            ProverStatus::Proved(_) => Err(anyhow::anyhow!(
                "Witness for block_header_hash {:?}, submitted multiple times.",
                block_header_hash,
            )
            .into()),
            ProverStatus::Err(e) => Err(e.into()),
        }
    }

    pub(crate) fn create_aggregated_proof(
        &self,
        jump: usize,
        block_header_hashes: &[<Da::Spec as DaSpec>::SlotHash],
    ) -> Result<ProofAggregationStatus, anyhow::Error> {
        assert!(jump >= 1);
        assert_eq!(block_header_hashes.len(), jump);
        let mut prover_state = self.prover_state.write().expect("Lock was poisoned");

        let mut block_proofs_data = Vec::default();

        for slot_hash in block_header_hashes {
            let state = prover_state.get_prover_status(slot_hash);

            match state {
                Some(ProverStatus::WitnessSubmitted(_)) => {
                    return Err(anyhow::anyhow!(
                    "Witness for {:?} was submitted, but the proof generation is not triggered.",
                    slot_hash
                ))
                }
                Some(ProverStatus::ProvingInProgress) => {
                    return Ok(ProofAggregationStatus::ProofGenerationInProgress)
                }
                Some(ProverStatus::Proved(block_proof)) => {
                    assert_eq!(slot_hash, &block_proof.st.slot_hash);
                    block_proofs_data.push(block_proof);
                }
                Some(ProverStatus::Err(e)) => return Err(anyhow::anyhow!(e.to_string())),
                None => return Err(anyhow::anyhow!("Missing witness for: {:?}", slot_hash)),
            }
        }

        // It is ok to unwrap here as we asserted that block_proofs_data.len() >= 1.
        let initial_block_proof = block_proofs_data.first().unwrap();
        let final_block_proof = block_proofs_data.last().unwrap();

        let public_input = AggregatedProofPublicInput {
            initial_state_root: initial_block_proof.st.initial_state_root.as_ref().to_vec(),
            final_state_root: final_block_proof.st.final_state_root.as_ref().to_vec(),
            initial_slot_hash: initial_block_proof.st.slot_hash.clone().into().to_vec(),
            final_slot_hash: final_block_proof.st.slot_hash.clone().into().to_vec(),
        };

        let info = AggregatedProofDataInfo {
            initial_slot_number: initial_block_proof.slot_number,
            final_slot_number: final_block_proof.slot_number,
        };

        let aggregated_proof = AggregatedProofData::new(public_input, info);

        for slot_hash in block_header_hashes {
            prover_state.remove(slot_hash);
        }
        Ok(ProofAggregationStatus::Success(aggregated_proof))
    }
}

fn make_proof<V, Vm, Da>(
    mut vm: Vm,
    config: Arc<RollupProverConfig>,
    zk_storage: V::PreState,
    stf_verifier: &StateTransitionVerifier<V, Da::Verifier, Vm::Guest>,
) -> Result<Proof, anyhow::Error>
where
    Da: DaService,
    Vm: ZkvmHost + 'static,
    V: StateTransitionFunction<Vm::Guest, Da::Spec> + Send + Sync + 'static,
    V::PreState: Send + Sync + 'static,
{
    match config.deref() {
        RollupProverConfig::Skip => Ok(Proof::PublicInput(Vec::default())),
        RollupProverConfig::Simulate => stf_verifier
            .run_block(vm.simulate_with_hints(), zk_storage)
            .map(|_| Proof::PublicInput(Vec::default()))
            .map_err(|e| anyhow::anyhow!("Guest execution must succeed but failed with {:?}", e)),
        RollupProverConfig::Execute => vm.run(false),
        RollupProverConfig::Prove => vm.run(true),
    }
}
