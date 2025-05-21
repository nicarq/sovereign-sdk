//! Implementations of [`sov_rollup_interface::storage::HierarchicalStorageManager`].
mod delta_reader_based;
mod nomt_based;
#[cfg(test)]
pub mod tests;

pub use delta_reader_based::*;
pub use nomt_based::{
    InitializableNativeNomtStorage, NomtChangeSet, NomtStateChangeSet, NomtStorageManager,
};
