use sov_rollup_interface::stf::ProofSerializer;
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;

use crate::{BlobData, Spec};

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
        let proof = BlobData::new_proof(serialized_proof.raw_aggregated_proof);
        let serialized_proof = borsh::to_vec(&proof)?;
        Ok(serialized_proof)
    }
}
