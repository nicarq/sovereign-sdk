use std::sync::Arc;

use sov_db::ledger_db::LedgerDb;
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::services::da::DaService;
use sov_rollup_interface::zk::aggregated_proof::{AggregatedProof, SerializedAggregatedProof};
use sov_rollup_interface::zk::Zkvm;
use tracing::{debug, info};
use types::{BlockProofInfo, BlockProofStatus, UnAggregatedProofList};

use self::types::AggregateProofMetadata;
use crate::config::ProofManagerConfig;
use crate::prover_service::AggregatedProofPublicData;
use crate::{ProverService, StateTransitionInfo};

mod types;

/// Manages the lifecycle of the `AggregatedProof`.
pub struct ProofManager<Ps: ProverService> {
    da_service: Arc<Ps::DaService>,
    prover_service: Option<Ps>,
    ledger_db: LedgerDb,
    outer_code_commitment: <Ps::Verifier as Zkvm>::CodeCommitment,
    proofs_to_create: UnAggregatedProofList<Ps>,
    config: ProofManagerConfig,
}

impl<Ps: ProverService> ProofManager<Ps>
where
    Ps::DaService: DaService<Error = anyhow::Error>,
{
    /// Creates a new proof manager.
    pub fn new(
        da_service: Arc<Ps::DaService>,
        prover_service: Option<Ps>,
        ledger_db: LedgerDb,
        outer_code_commitment: <Ps::Verifier as Zkvm>::CodeCommitment,
        config: ProofManagerConfig,
    ) -> Self {
        Self {
            da_service,
            prover_service,
            ledger_db,
            outer_code_commitment,
            proofs_to_create: UnAggregatedProofList::new(),
            config,
        }
    }

    /// Stores the `AggregatedProof` posted on DA into the database.
    pub(crate) async fn save_aggregated_proof(&self, height: u64) -> Result<(), anyhow::Error> {
        let aggregated_proofs = self.da_service.get_aggregated_proofs_at(height).await?;
        info!(%height, num_proofs=aggregated_proofs.len(), "Saving available aggregated proofs");
        for raw_aggregated_proof in aggregated_proofs {
            // Verify aggregated proof before storing it into the database.
            let public_data: AggregatedProofPublicData = match <Ps::Verifier as Zkvm>::verify(
                &raw_aggregated_proof,
                &self.outer_code_commitment,
            ) {
                Ok(public_data) => public_data,
                Err(err) => {
                    debug!(?err, "Received invalid aggregated proof for the DA");
                    return Ok(());
                }
            };

            self.ledger_db
                .save_finalized_aggregated_proof(AggregatedProof::new(
                    SerializedAggregatedProof {
                        raw_aggregated_proof,
                    },
                    public_data,
                ))?;
        }

        Ok(())
    }

    /// Attempts to generate an `AggregatedProof` and then posts it to DA.
    /// The proof is created only when there are enough of inner proofs in the `ProverService`` queue.
    pub(crate) async fn post_aggregated_proof_to_da_when_ready(
        &mut self,
        transition_data: StateTransitionInfo<
            Ps::StateRoot,
            Ps::Witness,
            <Ps::DaService as DaService>::Spec,
        >,
    ) -> Result<(), anyhow::Error> {
        if let Some(prover_service) = self.prover_service.as_ref() {
            let block_hash = transition_data.da_block_header().hash();
            // Save the transition for later proving. This is temporarily redundant
            // since we always just try to prove blocks right away (because we don't have fee
            // estimates for proving built out yet).
            self.proofs_to_create.append(BlockProofInfo {
                status: BlockProofStatus::Waiting(transition_data),
                hash: block_hash,
                // TODO(@preston-evans98): estimate public data size. This requires a new API on the `prover_service`.
                // <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/440>
                public_data_size: 0,
            });

            // Start proving the next block right away... for now.
            self.proofs_to_create
                .oldest_mut()
                .prove_any_unproven_blocks(prover_service)
                .await;

            // If we've covered enough blocks for the aggregate proof, generate and submit it to DA
            if self.proofs_to_create.current_proof_jump() >= self.config.aggregated_proof_block_jump
            {
                self.proofs_to_create.close_newest_proof();
                let metadata = self.proofs_to_create.take_oldest();
                let agg_proof = self
                    .create_aggregate_proof_with_retries(metadata, prover_service)
                    .await;
                tracing::debug!(
                    bytes = agg_proof.raw_aggregated_proof.len(),
                    "Sending aggregated proof to DA"
                );
                self.da_service
                    .send_aggregated_zk_proof(&agg_proof.raw_aggregated_proof)
                    .await?;
            }
        }
        Ok(())
    }

    async fn create_aggregate_proof_with_retries(
        &self,
        mut metadata: AggregateProofMetadata<Ps>,
        prover_service: &Ps,
    ) -> SerializedAggregatedProof {
        // TODO: Add backoff on proof submission
        // <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/446>
        loop {
            match metadata.prove(prover_service).await {
                Ok(proof) => break proof,
                Err((returned_metadata, err)) => {
                    tracing::error!("Failed to generate aggregate proof: {}. Retrying.", err);
                    metadata = returned_metadata;
                }
            }
        }
    }
}
