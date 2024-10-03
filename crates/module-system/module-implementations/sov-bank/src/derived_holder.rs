//! A derived holder is an entity that can be programmatically generated from a `[u8; 32]` array.
use sov_modules_api::impl_hash32_type;

impl_hash32_type!(DerivedHolder, DerivedHolderBech32, "derived_");
