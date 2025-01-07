//! Standard implementation of [`ProofSerializer`].

use std::sync::Arc;

use async_trait::async_trait;
use borsh::{BorshDeserialize, BorshSerialize};
use sov_modules_api::capabilities::config_chain_id;
use sov_modules_api::proof_metadata::{ProofType, SerializeProofWithDetails};
use sov_modules_api::transaction::{PriorityFeeBips, TxDetails};
use sov_modules_api::{ProofSerializer, Spec};
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::optimistic::{SerializedAttestation, SerializedChallenge};
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;
use sov_sequencer::SequenceNumberProvider;

const MAX_FEE: u64 = 10_000_000;

/// Adds metadata about gas & fees to the proof blob.
pub struct SovApiProofSerializer<S: Spec> {
    _phantom: std::marker::PhantomData<S>,
    sequence_number_provider: Option<Arc<dyn SequenceNumberProvider>>,
}

impl<S: Spec> SovApiProofSerializer<S> {
    /// Creates a new [`SovApiProofSerializer`].
    ///
    /// If `sequence_number_provider` is [`Some`], the proof serializer will
    /// produce preferred blobs.
    pub fn new(sequence_number_provider: Option<Arc<dyn SequenceNumberProvider>>) -> Self {
        Self {
            _phantom: Default::default(),
            sequence_number_provider,
        }
    }

    async fn serialize_proof_with_details(
        &self,
        proof_with_details: &SerializeProofWithDetails<S>,
    ) -> anyhow::Result<Vec<u8>> {
        let data = borsh::to_vec(&proof_with_details)?;

        if let Some(ref provider) = self.sequence_number_provider {
            let sequence_number = provider.generate_sequence_number(&data).await?;

            let bytes = borsh::to_vec(&PreferredProofData {
                sequence_number,
                data,
            })?;

            Ok(bytes)
        } else {
            // TODO: Put SerializedAggregatedProof directly on chain without wrapping in a vec <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1065>
            Ok(borsh::to_vec(&data)?)
        }
    }
}

#[async_trait]
impl<S: Spec> ProofSerializer for SovApiProofSerializer<S> {
    async fn serialize_proof_blob_with_metadata(
        &self,
        serialized_proof: SerializedAggregatedProof,
    ) -> anyhow::Result<Vec<u8>> {
        let details: TxDetails<S> = make_details(MAX_FEE);

        let proof_with_details = SerializeProofWithDetails {
            proof: ProofType::ZkAggregatedProof(serialized_proof),
            details,
        };

        Ok(self
            .serialize_proof_with_details(&proof_with_details)
            .await?)
    }

    async fn serialize_attestation_blob_with_metadata(
        &self,
        serialized_attestation: SerializedAttestation,
    ) -> anyhow::Result<Vec<u8>> {
        let details: TxDetails<S> = make_details(MAX_FEE);

        let proof_with_details = SerializeProofWithDetails {
            proof: ProofType::OptimisticProofAttestation(serialized_attestation),
            details,
        };

        Ok(self
            .serialize_proof_with_details(&proof_with_details)
            .await?)
    }

    async fn serialize_challenge_blob_with_metadata(
        &self,
        serialized_challenge: SerializedChallenge,
        slot_height: SlotNumber,
    ) -> anyhow::Result<Vec<u8>> {
        let details: TxDetails<S> = make_details(MAX_FEE);

        let proof_with_details = SerializeProofWithDetails {
            proof: ProofType::OptimisticProofChallenge(serialized_challenge, slot_height),
            details,
        };

        Ok(self
            .serialize_proof_with_details(&proof_with_details)
            .await?)
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

#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize)]
struct PreferredProofData {
    sequence_number: u64,
    data: Vec<u8>,
}
