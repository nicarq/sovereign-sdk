use sov_rollup_interface::da::DaSpec;

use crate::{Spec, StateCheckpoint};

/// The `ProofProcessor` capability is responsible for processing zk-proofs inside
/// the stf-blueprint.
pub trait ProofProcessor<S: Spec, Da: DaSpec> {
    /// Called by the stf once the proof is received.
    fn process_proof(&self, proof_batch: Vec<u8>, state: StateCheckpoint<S>) -> StateCheckpoint<S>;
}
