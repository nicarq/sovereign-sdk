use sov_rollup_interface::optimistic::{SerializedAttestation, SerializedChallenge};
use sov_rollup_interface::stf::ProofSerializer;
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;

use crate::capabilities::config_chain_id;
use crate::transaction::{PriorityFeeBips, TxDetails};
use crate::Spec;

/// Proof type supported by the rollup.

#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ProofType {
    /// ZK workflow: aggregated zk proof.
    ZkAggregatedProof(SerializedAggregatedProof),
    /// Optimistic workflow: attestation.
    OptimisticProofAttestation(SerializedAttestation),
    /// Optimistic workflow: challenge.
    OptimisticProofChallenge(SerializedChallenge, u64),
}

/// Proof with metadata need for verification.
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct SerializeProofWithDetails<S: Spec> {
    /// The serialized aggregated proof.
    pub proof: ProofType,
    /// The transaction metadata.
    pub details: TxDetails<S>,
}

/// Adds metadata about gas & fees to the proof blob.
pub struct SovApiProofSerializer<S: Spec> {
    _phantom: std::marker::PhantomData<S>,
}

const MAX_FEE: u64 = 10_000_000;

impl<S: Spec> ProofSerializer for SovApiProofSerializer<S> {
    fn new() -> Self
    where
        Self: Sized,
    {
        SovApiProofSerializer {
            _phantom: Default::default(),
        }
    }

    fn serialize_proof_blob_with_metadata(
        &self,
        serialized_proof: SerializedAggregatedProof,
    ) -> anyhow::Result<Vec<u8>> {
        let details: TxDetails<S> = make_details(MAX_FEE);

        let proof_with_details = SerializeProofWithDetails {
            proof: ProofType::ZkAggregatedProof(serialized_proof),
            details,
        };

        let serialized_proof_with_details = serialize_proof_with_details(&proof_with_details)?;

        Ok(serialized_proof_with_details)
    }

    fn serialize_attestation_blob_with_metadata(
        &self,
        serialized_attestation: SerializedAttestation,
    ) -> anyhow::Result<Vec<u8>> {
        let details: TxDetails<S> = make_details(MAX_FEE);

        let proof_with_details = SerializeProofWithDetails {
            proof: ProofType::OptimisticProofAttestation(serialized_attestation),
            details,
        };

        let serialized_proof_with_details = serialize_proof_with_details(&proof_with_details)?;

        Ok(serialized_proof_with_details)
    }

    fn serialize_challenge_blob_with_metadata(
        &self,
        serialized_challenge: SerializedChallenge,
        slot_height: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let details: TxDetails<S> = make_details(MAX_FEE);

        let proof_with_details = SerializeProofWithDetails {
            proof: ProofType::OptimisticProofChallenge(serialized_challenge, slot_height),
            details,
        };

        let serialized_proof_with_details = serialize_proof_with_details(&proof_with_details)?;

        Ok(serialized_proof_with_details)
    }
}

fn make_details<S: Spec>(max_fee: u64) -> TxDetails<S> {
    TxDetails {
        max_priority_fee_bips: PriorityFeeBips::ZERO,
        max_fee,
        gas_limit: None,
        chain_id: config_chain_id(),
    }
}

fn serialize_proof_with_details<S: Spec>(
    proof_with_details: &SerializeProofWithDetails<S>,
) -> anyhow::Result<Vec<u8>> {
    // TODO: Put SerializedAggregatedProof directly on chain without wrapping in a vec <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1065>
    Ok(borsh::to_vec(&borsh::to_vec(&proof_with_details)?)?)
}
