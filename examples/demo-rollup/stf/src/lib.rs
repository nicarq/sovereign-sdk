#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

pub mod authentication;
#[cfg(feature = "native")]
pub mod genesis_config;
mod hooks_impl;
pub mod runtime;
#[cfg(test)]
mod tests;

use sov_modules_stf_blueprint::StfBlueprint;
use sov_rollup_interface::stf::StateTransitionVerifier;

/// Alias for StateTransitionVerifier.
pub type StfVerifier<DA, ZkSpec, RT, K, InnerVm, OuterVm> =
    StateTransitionVerifier<StfBlueprint<ZkSpec, RT, K>, DA, InnerVm, OuterVm>;
