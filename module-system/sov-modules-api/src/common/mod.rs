//! Common types shared between state and modules

mod address;

mod module_id;
pub use module_id::{ModuleId, ModuleIdBech32};

mod error;
mod gas;
mod key;

pub use address::*;
pub use error::*;
pub use gas::*;
#[cfg(feature = "std")]
pub use jmt::Version;
pub use key::*;

/// The version of the JellyfishMerkleTree state.
#[cfg(not(feature = "std"))]
pub type Version = u64;
