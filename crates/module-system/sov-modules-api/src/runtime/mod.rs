//! Module system runtime types and traits
pub mod capabilities;

use borsh::{BorshDeserialize, BorshSerialize};
use capabilities::{HasCapabilities, HasKernel, TransactionAuthenticator};
use serde::{Deserialize, Serialize};

use crate::hooks::{ApplyBatchHooks, FinalizeHook, KernelSlotHooks, SlotHooks, TxHooks};
use crate::{BatchSequencerReceipt, DispatchCall, Genesis, RuntimeEventProcessor, Spec};

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

/// This trait has to be implemented by a runtime in order to be used in `StfBlueprint`.
///
/// The `TxHooks` implementation sets up a transaction context based on the height at which it is
/// to be executed.
pub trait Runtime<S: Spec>:
    DispatchCall<Spec = S>
    + HasCapabilities<S>
    + HasKernel<S>
    + TransactionAuthenticator<
        S,
        Decodable = <Self as DispatchCall>::Decodable,
        AuthorizationData = <Self as HasCapabilities<S>>::AuthorizationData,
    > + Genesis<Spec = S, Config = Self::GenesisConfig>
    + TxHooks<Spec = S>
    + SlotHooks<Spec = S>
    + KernelSlotHooks<Spec = S>
    + FinalizeHook<Spec = S>
    + ApplyBatchHooks<Spec = S, BatchResult = BatchSequencerReceipt<S>>
    + Default
    + RuntimeEventProcessor
    + 'static
{
    /// Chain root hash used for transaction verification. Generated from a
    /// [schema](sov_rollup_interface::sov_universal_wallet::schema::Schema).
    const CHAIN_HASH: [u8; 32];

    /// GenesisConfig type.
    type GenesisConfig: Send + Sync;

    /// GenesisPaths type.
    #[cfg(feature = "native")]
    type GenesisPaths: Send + Sync;

    /// Default RPC methods and Axum router.
    #[cfg(feature = "native")]
    fn endpoints(storage: crate::rest::ApiState<S>) -> RuntimeEndpoints;

    /// Reads genesis configs.
    #[cfg(feature = "native")]
    fn genesis_config(genesis_paths: &Self::GenesisPaths) -> anyhow::Result<Self::GenesisConfig>;
}

/// The return type of [`Runtime::endpoints`].
#[cfg(feature = "native")]
pub struct RuntimeEndpoints {
    /// The [`axum::Router`] for the runtime's HTTP server.
    pub axum_router: axum::Router<()>,
    /// A [`jsonrpsee::RpcModule`] for the runtime's JSON-RPC server.
    pub jsonrpsee_module: jsonrpsee::RpcModule<()>,
}

#[cfg(feature = "native")]
impl Default for RuntimeEndpoints {
    fn default() -> Self {
        Self {
            axum_router: Default::default(),
            jsonrpsee_module: jsonrpsee::RpcModule::new(()),
        }
    }
}
