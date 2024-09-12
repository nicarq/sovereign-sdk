//! Module system runtime types and traits
pub mod capabilities;
pub mod kernel_module;

use borsh::{BorshDeserialize, BorshSerialize};
pub use kernel_module::KernelModule;
use serde::{Deserialize, Serialize};

/// Flag indicating what mode the rollup is operating in.
#[derive(
    BorshDeserialize, BorshSerialize, Serialize, Deserialize, Debug, PartialEq, Eq, Copy, Clone,
)]
#[serde(rename_all = "snake_case")]
pub enum OperatingMode {
    /// The rollup is currently executing in optimistic mode.
    Optimistic,
    /// The rollup is currently executing in zk mode.
    Zk,
}
