#[cfg(feature = "native")]
use sov_rollup_interface::optimistic::BondingProofService;
use sov_rollup_interface::optimistic::{SerializedAttestation, SerializedChallenge};
use sov_rollup_interface::stf::InvalidProofError;
use sov_rollup_interface::zk::aggregated_proof::{
    AggregatedProofPublicData, SerializedAggregatedProof,
};

#[cfg(feature = "native")]
use super::HasKernel;
use crate::{SovAttestation, SovStateTransitionPublicData, Spec, TxState};

/// The `ProofProcessor` capability is responsible for processing proofs inside
/// the stf-blueprint.
pub trait ProofProcessor<S: Spec> {
    /// The service that generates bonding proofs for attesters.
    #[cfg(feature = "native")]
    type BondingProofService<K: HasKernel<S>>: BondingProofService;

    /// Creates a new [`BondingProofService`] service.
    #[cfg(feature = "native")]
    fn create_bonding_proof_service<K: HasKernel<S>>(
        &self,
        attester_address: <S as Spec>::Address,
        storage: tokio::sync::watch::Receiver<<S as Spec>::Storage>,
        kernel: K,
    ) -> Self::BondingProofService<K>;

    /// Called by the stf once the zk-proof is received.
    fn process_aggregated_proof(
        &self,
        proof: SerializedAggregatedProof,
        prover_address: &S::Address,
        state: &mut impl TxState<S>,
    ) -> Result<(AggregatedProofPublicData, SerializedAggregatedProof), InvalidProofError>;

    /// Called by the stf once the attestation is received.
    fn process_attestation(
        &self,
        proof: SerializedAttestation,
        prover_address: &S::Address,
        state: &mut impl TxState<S>,
    ) -> Result<SovAttestation<S>, InvalidProofError>;

    /// Called by the stf once the challenge is received.
    fn process_challenge(
        &self,
        proof: SerializedChallenge,
        rollup_height: u64,
        prover_address: &S::Address,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<SovStateTransitionPublicData<S>, InvalidProofError>;
}
