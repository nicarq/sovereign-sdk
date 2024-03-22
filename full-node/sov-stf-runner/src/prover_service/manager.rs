use std::sync::Arc;
use std::time::Duration;

use sov_db::ledger_db::LedgerDB;
use sov_rollup_interface::da::{BlockHeaderTrait, DaSpec};
use sov_rollup_interface::services::da::DaService;
use sov_rollup_interface::zk::aggregated_proof::{AggregatedProofData, SerializedAggregatedProof};
use sov_rollup_interface::zk::Zkvm;
use tracing::{debug, info};

use crate::prover_service::AggregatedProofPublicInput;
use crate::{ProofAggregationStatus, ProofProcessingStatus, ProverService, StateTransitionInfo};

/// Manages the lifecycle of the `AggregatedProof`.
pub struct ProofManager<Ps: ProverService> {
    da_service: Arc<Ps::DaService>,
    prover_service: Option<Ps>,
    ledger_db: LedgerDB,
    outer_code_commitment: <Ps::Verifier as Zkvm>::CodeCommitment,
}

impl<Ps: ProverService> ProofManager<Ps>
where
    Ps::DaService: DaService<Error = anyhow::Error>,
{
    /// Creates a new proof manager.
    pub fn new(
        da_service: Arc<Ps::DaService>,
        prover_service: Option<Ps>,
        ledger_db: LedgerDB,
        outer_code_commitment: <Ps::Verifier as Zkvm>::CodeCommitment,
    ) -> Self {
        Self {
            da_service,
            prover_service,
            ledger_db,
            outer_code_commitment,
        }
    }

    /// Stores the `AggregatedProof` posted on DA into the database.
    pub(crate) async fn save_aggregated_proof(&self, height: u64) -> Result<(), anyhow::Error> {
        info!(%height, "Saving aggregated proof");
        let aggregated_proofs = self.da_service.get_aggregated_proofs_at(height).await?;
        for raw_aggregated_proof in aggregated_proofs {
            // Verify aggregated proof before storing it into the database.
            let public_input: AggregatedProofPublicInput = match <Ps::Verifier as Zkvm>::verify(
                &raw_aggregated_proof,
                &self.outer_code_commitment,
            ) {
                Ok(public_input) => public_input,
                Err(err) => {
                    debug!(?err, "Received invalid aggregated proof for the DA");
                    return Ok(());
                }
            };

            self.ledger_db
                .save_finalized_aggregated_proof(AggregatedProofData::new(
                    SerializedAggregatedProof {
                        raw_aggregated_proof,
                    },
                    public_input,
                ))?;
        }

        Ok(())
    }

    /// Attempts to generate an `AggregatedProof` and then posts it to DA.
    /// The proof is created only when there are enough of inner proofs in the `ProverService`` queue.
    pub(crate) async fn post_aggregated_proof_to_da_when_ready(
        &self,
        transition_data: StateTransitionInfo<
            Ps::StateRoot,
            Ps::Witness,
            <Ps::DaService as DaService>::Spec,
        >,
        agg_proof_hashes: &mut Vec<<<Ps::DaService as DaService>::Spec as DaSpec>::SlotHash>,
    ) -> Result<(), anyhow::Error> {
        if let Some(prover_service) = self.prover_service.as_ref() {
            let header_hash = transition_data.da_block_header().hash();
            agg_proof_hashes.push(header_hash.clone());

            prover_service
                .submit_state_transition_info(transition_data)
                .await;

            loop {
                let status = prover_service
                    .prove(header_hash.clone())
                    .await
                    .expect("The proof creation should succeed");

                // Stop the runner loop until prover is ready.
                match status {
                    ProofProcessingStatus::ProvingInProgress => break,
                    ProofProcessingStatus::Busy => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                }
            }

            if agg_proof_hashes.len() >= prover_service.aggregated_proof_block_jump() {
                loop {
                    let status = prover_service
                        .create_aggregated_proof(agg_proof_hashes.as_slice())
                        .await;

                    match status {
                        Ok(ProofAggregationStatus::Success(agg_proof)) => {
                            agg_proof_hashes.clear();

                            tracing::debug!(
                                bytes = agg_proof.raw_aggregated_proof.len(),
                                "Sending aggregated proof to DA"
                            );
                            self.da_service
                                .send_aggregated_zk_proof(&agg_proof.raw_aggregated_proof)
                                .await?;
                            return Ok(());
                        }
                        // TODO(https://github.com/Sovereign-Labs/sovereign-sdk/issues/1185): Add timeout handling.
                        Ok(ProofAggregationStatus::ProofGenerationInProgress) => {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        // TODO(https://github.com/Sovereign-Labs/sovereign-sdk/issues/1185): Add handling for DA submission errors.
                        Err(e) => panic!("{:?}", e),
                    }
                }
            }
        }
        Ok(())
    }
}
