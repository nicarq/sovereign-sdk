//! Storage and state management interfaces for Sovereign SDK modules.

#![deny(missing_docs)]

pub mod codec;

#[cfg(feature = "native")]
mod prover_storage;

/// Defines the data structures needed by both the zk-storage and the prover storage.
mod storage_internals;

pub use storage_internals::{SparseMerkleProof, StorageRoot, VisibleHash};

mod witness;
mod zk_storage;
pub mod jmt {
    //! Re-export the [`jellyfish-merkle-tree`](https://github.com/penumbra-zone/jmt) crate.
    pub use jmt::{KeyHash, RootHash};
}
#[cfg(feature = "native")]
pub use prover_storage::{ProverChangeSet, ProverStorage};
pub use zk_storage::ZkStorage;

pub mod config;
pub use sov_modules_core::{
    storage, AlignedVec, OrderedReadsAndWrites, Prefix, ProvableStorageCache, Storage, Witness,
};
use sov_rollup_interface::digest::Digest;

pub use crate::witness::ArrayWitness;

/// A trait specifying the hash function and format of the witness used in
/// merkle proofs for storage access
pub trait MerkleProofSpec: Send + Sync {
    /// The structure that accumulates the witness data
    type Witness: Witness + Send + Sync;
    /// The hash function used to compute the merkle root
    type Hasher: Digest<OutputSize = sha2::digest::typenum::U32> + Send + Sync;
}

use sha2::Sha256;

/// The default [`MerkleProofSpec`] implementation.
///
/// This type is typically found as a type parameter for [`ProverStorage`].
#[derive(Clone)]
pub struct DefaultStorageSpec;
// TODO(@preston-evans98): Make this type generic over a hasher <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/188>

impl MerkleProofSpec for DefaultStorageSpec {
    type Witness = ArrayWitness;

    type Hasher = Sha256;
}
