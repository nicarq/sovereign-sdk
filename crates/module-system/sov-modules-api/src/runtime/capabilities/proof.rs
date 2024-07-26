use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::stf::ProofOutcome;
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;
use sov_state::Storage;

use crate::{Spec, WorkingSet};

/// The `ProofProcessor` capability is responsible for processing zk-proofs inside
/// the stf-blueprint.
pub trait ProofProcessor<S: Spec, Da: DaSpec> {
    /// Called by the stf once the proof is received.
    fn process_proof(
        &self,
        proof: &SerializedAggregatedProof,
        prover_address: &S::Address,
        state: &mut WorkingSet<S>,
    ) -> ProofOutcome<S::Address, Da, <S::Storage as Storage>::Root>;
}
