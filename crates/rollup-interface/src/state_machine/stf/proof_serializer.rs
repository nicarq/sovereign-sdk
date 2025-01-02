use async_trait::async_trait;

use crate::common::SlotNumber;
use crate::optimistic::{SerializedAttestation, SerializedChallenge};
use crate::zk::aggregated_proof::SerializedAggregatedProof;

/// Serialize proof blob and adds metadata needed for verification.
#[async_trait]
pub trait ProofSerializer: Send + Sync {
    /// Creates a proof blob with metadata needed for verification.
    async fn serialize_proof_blob_with_metadata(
        &self,
        serialized_proof: SerializedAggregatedProof,
    ) -> anyhow::Result<Vec<u8>>;

    /// Creates an attestation blob with metadata needed for verification.
    async fn serialize_attestation_blob_with_metadata(
        &self,
        serialized_attestation: SerializedAttestation,
    ) -> anyhow::Result<Vec<u8>>;

    /// Creates a challenge blob with metadata needed for verification.
    async fn serialize_challenge_blob_with_metadata(
        &self,
        serialized_challenge: SerializedChallenge,
        slot_height: SlotNumber,
    ) -> anyhow::Result<Vec<u8>>;
}
