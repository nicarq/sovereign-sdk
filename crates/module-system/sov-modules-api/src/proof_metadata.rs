use sov_modules_macros::config_value;
use sov_rollup_interface::optimistic::{SerializedAttestation, SerializedChallenge};
use sov_rollup_interface::stf::ProofSerializer;
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;

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
pub enum ProofType {
    /// ZK workflow: aggregated zk proof.
    ZkAggregatedProof(SerializedAggregatedProof),
    /// Optimistic workflow: attestation.
    OptimisticProofAttestation(SerializedAttestation),
    /// Optimistic workflow: challenge.
    OptimisticProofChallenge(SerializedChallenge),
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
        let details = TxDetails::<S> {
            max_priority_fee_bips: PriorityFeeBips::ZERO,
            max_fee: 10_000_000,
            gas_limit: None,
            chain_id: config_value!("CHAIN_ID"),
        };

        let proof_with_details = SerializeProofWithDetails {
            proof: ProofType::ZkAggregatedProof(serialized_proof),
            details,
        };

        // TODO: Put SerializedAggregatedProof directly on chain without wrapping in a vec <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1065>
        let serialized_proof_with_details = borsh::to_vec(&borsh::to_vec(&proof_with_details)?)?;

        Ok(serialized_proof_with_details)
    }
}
