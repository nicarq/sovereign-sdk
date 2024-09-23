//! Common types shared between state and modules

mod address;

mod module_id;

pub use module_id::{ModuleId, ModuleIdBech32};

mod crypto;
mod error;

pub use address::*;
pub use crypto::*;
pub use error::*;
pub use sov_state::jmt::Version;

/// Implement the `arbitrary::Arbitrary` trait when the `arbitrary` feature is enabled.
#[cfg(feature = "arbitrary")]
pub trait MaybeArbitrary: for<'a> arbitrary::Arbitrary<'a> {}
#[cfg(feature = "arbitrary")]
impl<T: for<'a> arbitrary::Arbitrary<'a>> MaybeArbitrary for T {}

/// Implement the `arbitrary::Arbitrary` trait when the `arbitrary` feature is enabled.
#[cfg(not(feature = "arbitrary"))]
pub trait MaybeArbitrary {}
#[cfg(not(feature = "arbitrary"))]
impl<T> MaybeArbitrary for T {}
