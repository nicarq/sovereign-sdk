use std::sync::Arc;

use borsh::BorshSerialize;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::optimistic::{Attestation, BondingProofService, SerializedAttestation};
use sov_rollup_interface::stf::ProofSerializer;
use tokio::task::JoinHandle;

use crate::processes::Receiver;

/// Manages the lifecycle of the [`Attestation`].
pub struct AttestationsManager<StateRoot, Witness, Da: DaService, Bps: BondingProofService> {
    st_info_receiver: Receiver<StateRoot, Witness, Da::Spec>,
    bonding_proof_service: Bps,
    proof_serializer: Box<dyn ProofSerializer>,
    da_service: Arc<Da>,
}

impl<StateRoot, Witness, Da, Bps> AttestationsManager<StateRoot, Witness, Da, Bps>
where
    Bps: BondingProofService,
    Da: DaService<Error = anyhow::Error>,
    StateRoot: BorshSerialize + Serialize + DeserializeOwned + Send + Sync + 'static,
    Witness: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    /// Creates a new [`AttestationsManager`]
    pub fn new(
        st_info_receiver: Receiver<StateRoot, Witness, Da::Spec>,
        bonding_proof_service: Bps,
        proof_serializer: Box<dyn ProofSerializer>,
        da_service: Arc<Da>,
    ) -> Self {
        Self {
            st_info_receiver,
            bonding_proof_service,
            da_service,
            proof_serializer,
        }
    }

    /// Starts a background task for `Attestation` generation.
    pub async fn post_attestation_to_da_in_background(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.post_attestation_to_da().await {
                tracing::error!(error = ?e, "Failed to post attestation to DA");
            }
        })
    }

    async fn post_attestation_to_da(mut self) -> anyhow::Result<()> {
        while let Some(stf_info) = self.st_info_receiver.read_next().await? {
            let height = stf_info.rollup_height;
            let witness = stf_info.witness();

            let attestation = Attestation {
                initial_state_root: witness.initial_state_root,
                slot_hash: witness.da_block_header.hash(),
                post_state_root: witness.final_state_root,
                proof_of_bond: self.bonding_proof_service.get_bonding_proof(height).ok_or(
                    anyhow::anyhow!("Cannot get bonding proof, storage is corrupted."),
                )?,
            };

            let serialized_attestation = SerializedAttestation::from_attestation(&attestation)?;

            let serialized_blob = self
                .proof_serializer
                .serialize_attestation_blob_with_metadata(serialized_attestation)
                .await?;

            let fee = self.da_service.estimate_fee(serialized_blob.len()).await?;

            self.da_service.send_proof(&serialized_blob, fee).await?;
        }

        Ok(())
    }
}
