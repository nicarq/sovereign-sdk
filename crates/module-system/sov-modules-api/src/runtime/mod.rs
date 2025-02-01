//! Module system runtime types and traits
pub mod capabilities;

#[cfg(feature = "native")]
use std::io;

use borsh::{BorshDeserialize, BorshSerialize};
use capabilities::{HasCapabilities, HasKernel, TransactionAuthenticator};
use serde::{Deserialize, Serialize};

#[cfg(feature = "native")]
use crate::hooks::FinalizeHook;
use crate::hooks::{BlockHooks, KernelSlotHooks, TxHooks};
use crate::{DispatchCall, Genesis, RuntimeEventProcessor, Spec};

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

#[cfg(feature = "native")]
/// This trait has to be implemented by a runtime in order to be used in `StfBlueprint`.
///
/// The `TxHooks` implementation sets up a transaction context based on the height at which it is
/// to be executed.
pub trait Runtime<S: Spec>:
    DispatchCall<Spec = S>
    + HasCapabilities<S>
    + HasKernel<S>
    + TransactionAuthenticator<S, Decodable = <Self as DispatchCall>::Decodable>
    + Genesis<Spec = S, Config = Self::GenesisConfig>
    + TxHooks<Spec = S>
    + BlockHooks<Spec = S>
    + KernelSlotHooks<Spec = S>
    + FinalizeHook<Spec = S>
    + Default
    + RuntimeEventProcessor
    + 'static
{
    /// Chain root hash used for transaction verification. Generated from a
    /// [schema](sov_rollup_interface::sov_universal_wallet::schema::Schema).
    const CHAIN_HASH: [u8; 32];

    /// GenesisConfig type.
    type GenesisConfig: Clone + Send + Sync;

    /// GenesisInput type.
    type GenesisInput: std::fmt::Debug + Clone + Send + Sync;

    /// Default RPC methods and Axum router.
    fn endpoints(storage: crate::rest::ApiState<S>) -> RuntimeEndpoints;

    /// Reads genesis configs.
    fn genesis_config(input: &Self::GenesisInput) -> anyhow::Result<Self::GenesisConfig>;

    /// Gets the operating mode of the runtime (Zk or Optimistic).
    fn operating_mode(genesis: &Self::GenesisConfig) -> OperatingMode;

    /// Decodes serialized call message.
    fn decode_call(
        serialized_message: &[u8],
    ) -> Result<<Self as DispatchCall>::Decodable, io::Error> {
        decode_borsh_serialized_message::<<Self as DispatchCall>::Decodable>(serialized_message)
    }
}

#[cfg(feature = "native")]
/// Decodes borsh serialized message.
pub fn decode_borsh_serialized_message<T: borsh::BorshDeserialize>(
    mut serialized_message: &[u8],
) -> Result<T, io::Error> {
    let res = T::deserialize(&mut serialized_message)?;

    if !serialized_message.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "the provided message contains dangling data",
        ));
    }

    Ok(res)
}

#[cfg(not(feature = "native"))]
/// This trait has to be implemented by a runtime in order to be used in `StfBlueprint`.
///
/// The `TxHooks` implementation sets up a transaction context based on the height at which it is
/// to be executed.
pub trait Runtime<S: Spec>:
    DispatchCall<Spec = S>
    + HasCapabilities<S>
    + HasKernel<S>
    + TransactionAuthenticator<S, Decodable = <Self as DispatchCall>::Decodable>
    + Genesis<Spec = S, Config = Self::GenesisConfig>
    + TxHooks<Spec = S>
    + BlockHooks<Spec = S>
    + KernelSlotHooks<Spec = S>
    + Default
    + RuntimeEventProcessor
    + 'static
{
    /// Chain root hash used for transaction verification. Generated from a
    /// [schema](sov_rollup_interface::sov_universal_wallet::schema::Schema).
    const CHAIN_HASH: [u8; 32];

    /// GenesisConfig type.
    type GenesisConfig: Clone + Send + Sync;

    /// Gets the operating mode of the runtime (Zk or Optimistic).
    fn operating_mode(genesis: &Self::GenesisConfig) -> OperatingMode;
}

/// The return type of [`Runtime::endpoints`].
#[cfg(feature = "native")]
pub struct RuntimeEndpoints {
    /// The [`axum::Router`] for the runtime's HTTP server.
    pub axum_router: axum::Router<()>,
    /// A [`jsonrpsee::RpcModule`] for the runtime's JSON-RPC server.
    pub jsonrpsee_module: jsonrpsee::RpcModule<()>,
    /// A list of optional background tasks that have been spawned for the endpoints' purposes.
    pub background_handles: Vec<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

#[cfg(feature = "native")]
impl Default for RuntimeEndpoints {
    fn default() -> Self {
        Self {
            axum_router: Default::default(),
            jsonrpsee_module: jsonrpsee::RpcModule::new(()),
            background_handles: Vec::new(),
        }
    }
}
