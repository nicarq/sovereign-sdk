use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::optimistic::{SerializedAttestation, SerializedChallenge};
use sov_rollup_interface::stf::InvalidProofError;
use sov_rollup_interface::zk::aggregated_proof::{
    AggregatedProofPublicData, SerializedAggregatedProof,
};

use crate::{SovProofOutcome, Spec, WorkingSet};

/// The `ProofProcessor` capability is responsible for processing proofs inside
/// the stf-blueprint.
pub trait ProofProcessor<S: Spec, Da: DaSpec> {
    /// Called by the stf once the zk-proof is received.
    fn process_aggregated_proof(
        &self,
        proof: SerializedAggregatedProof,
        prover_address: &S::Address,
        state: &mut WorkingSet<S>,
    ) -> Result<(AggregatedProofPublicData, SerializedAggregatedProof), InvalidProofError>;

    /// Called by the stf once the attestation is received.
    fn process_attestation(
        &self,
        proof: SerializedAttestation,
        prover_address: &S::Address,
        state: &mut WorkingSet<S>,
    ) -> SovProofOutcome<S, Da>;

    /// Called by the stf once the challenge is received.
    fn process_challenge(
        &self,
        proof: SerializedChallenge,
        transition_num: u64,
        prover_address: &S::Address,
        state: &mut WorkingSet<S>,
    ) -> SovProofOutcome<S, Da>;
}
