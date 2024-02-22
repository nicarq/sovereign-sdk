#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

#[cfg(feature = "native")]
pub mod genesis_config;
mod hooks_impl;
pub mod runtime;
#[cfg(test)]
mod tests;

use sov_modules_stf_blueprint::StfBlueprint;
use sov_rollup_interface::da::DaVerifier;
use sov_rollup_interface::zk::ZkvmGuest;
use sov_stf_runner::verifier::StateTransitionVerifier;

/// Alias for StateTransitionVerifier.
pub type StfVerifier<DA, Vm, ZkSpec, RT, K> = StateTransitionVerifier<
    StfBlueprint<ZkSpec, <DA as DaVerifier>::Spec, <Vm as ZkvmGuest>::Verifier, RT, K>,
    DA,
    Vm,
>;
