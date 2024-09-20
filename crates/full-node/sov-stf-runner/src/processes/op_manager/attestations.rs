use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_rollup_interface::da::{BlockHeaderTrait, DaSpec};
use sov_rollup_interface::optimistic::{Attestation, BondingProofService};
use tokio::task::JoinHandle;

use crate::processes::Receiver;

/// Manages the lifecycle of the [`Attestation`].
pub struct AttestationsManager<StateRoot, Witness, Da: DaSpec, Bps: BondingProofService> {
    st_info_receiver: Receiver<StateRoot, Witness, Da>,
    bonding_proof_service: Bps,
}

impl<StateRoot, Witness, Da, Bps> AttestationsManager<StateRoot, Witness, Da, Bps>
where
    Bps: BondingProofService,
    Da: DaSpec,
    StateRoot: Serialize + DeserializeOwned + Send + Sync + 'static,
    Witness: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    /// Creates a new [`AttestationsManager`]
    pub fn new(
        st_info_receiver: Receiver<StateRoot, Witness, Da>,
        bonding_proof_service: Bps,
    ) -> Self {
        Self {
            st_info_receiver,
            bonding_proof_service,
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

            let _attestation = Attestation {
                initial_state_root: witness.initial_state_root,
                slot_hash: witness.da_block_header.hash(),
                post_state_root: witness.final_state_root,
                proof_of_bond: self.bonding_proof_service.get_bonding_proof(height),
            };
        }

        Ok(())
    }
}
