use std::io;

use borsh::BorshDeserialize;
use sov_rollup_interface::optimistic::{SerializedAttestation, SerializedChallenge};
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;

use crate::transaction::TxDetails;
use crate::{GasMeter, GasSpec, MeteredBorshDeserialize, MeteredBorshDeserializeError, Spec};

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
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(
    feature = "native",
    derive(borsh::BorshSerialize, serde::Serialize, serde::Deserialize,)
)]

pub struct SerializeProofWithDetails<S: Spec> {
    /// The serialized aggregated proof.
    pub proof: ProofType,
    /// The transaction metadata.
    pub details: TxDetails<S>,
}

impl<S: Spec> SerializeProofWithDetails<S> {
    fn unmetered_deserialize_inner(buf: &mut &[u8]) -> Result<Self, io::Error> {
        let signature = <ProofType as BorshDeserialize>::deserialize(buf)?;
        let pub_key = <TxDetails<S> as BorshDeserialize>::deserialize(buf)?;

        Ok(Self {
            proof: signature,
            details: pub_key,
        })
    }
}

impl<S: Spec> MeteredBorshDeserialize<S> for SerializeProofWithDetails<S> {
    fn deserialize(
        buf: &mut &[u8],
        meter: &mut impl GasMeter<<S as GasSpec>::Gas>,
    ) -> Result<Self, MeteredBorshDeserializeError<<S as GasSpec>::Gas>> {
        meter
            .charge_gas(&Self::gas_cost_to_deserialize(buf))
            .map_err(MeteredBorshDeserializeError::GasError)?;

        SerializeProofWithDetails::<S>::unmetered_deserialize_inner(buf)
            .map_err(MeteredBorshDeserializeError::IOError)
    }

    #[cfg(feature = "native")]
    fn unmetered_deserialize(
        buf: &mut &[u8],
    ) -> Result<Self, MeteredBorshDeserializeError<<S as GasSpec>::Gas>> {
        SerializeProofWithDetails::<S>::unmetered_deserialize_inner(buf)
            .map_err(MeteredBorshDeserializeError::IOError)
    }
}
