#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod call;
mod evm;
mod genesis;
mod hooks;

pub use call::*;
pub use evm::*;
pub use genesis::*;

#[cfg(feature = "native")]
mod rpc;

#[cfg(feature = "native")]
pub use rpc::*;

#[cfg(test)]
mod tests;

mod authenticate;
mod event;
#[cfg(feature = "native")]
mod helpers;

pub use authenticate::authenticate;
pub use reth_primitives::revm_primitives::SpecId;
use reth_primitives::revm_primitives::{Address, BlockEnv, B256};
pub use reth_primitives::Address as EvmAddress;
use sov_modules_api::prelude::UnwrapInfallible as _;
use sov_modules_api::{
    AccessoryStateReader, AccessoryStateReaderAndWriter, Context, Error, GenesisState,
    InfallibleStateReaderAndWriter, ModuleId, ModuleInfo, StateAccessor, StateReader, TxState,
    UnmeteredStateWrapper,
};
use sov_state::codec::BcsCodec;
use sov_state::User;

use crate::event::Event;
use crate::evm::db::EvmDb;
use crate::evm::primitive_types::{Block, Receipt, SealedBlock, TransactionSignedAndRecovered};

// Gas per transaction not creating a contract.
#[cfg(feature = "native")]
pub(crate) const MIN_TRANSACTION_GAS: u64 = 21_000u64;
#[cfg(feature = "native")]
pub(crate) const MIN_CREATE_GAS: u64 = 53_000u64;

/// A pending Ethereum transaction.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingTransaction {
    pub(crate) transaction: TransactionSignedAndRecovered,
    pub(crate) receipt: Receipt,
}

/// The sov-evm module provides compatibility with the EVM.
#[allow(dead_code)]
#[derive(Clone, ModuleInfo)]
pub struct Evm<S: sov_modules_api::Spec> {
    /// The ID of the evm module.
    #[id]
    pub(crate) id: ModuleId,

    /// Mapping from account address to account state.
    #[state]
    pub(crate) accounts: sov_modules_api::StateMap<Address, DbAccount, BcsCodec>,

    /// Mapping from code hash to code. Used for lazy-loading code into a contract account.
    #[state]
    pub(crate) code: sov_modules_api::StateMap<B256, reth_primitives::Bytes, BcsCodec>,

    /// Chain configuration. This field is set in genesis.
    #[state]
    pub(crate) cfg: sov_modules_api::StateValue<EvmChainConfig, BcsCodec>,

    /// Block environment used by the evm. This field is set in `begin_slot_hook`.
    #[state]
    pub(crate) block_env: sov_modules_api::StateValue<BlockEnv, BcsCodec>,

    /// Transactions that will be added to the current block.
    /// A valid transaction is added to the vec on every call message.
    #[state]
    pub(crate) pending_transactions: sov_modules_api::StateVec<PendingTransaction, BcsCodec>,

    /// Head of the chain. The new head is set in `end_slot_hook` but without the inclusion of the `state_root` field.
    /// The `state_root` is added in `begin_slot_hook` of the next block because its calculation occurs after the `end_slot_hook`.
    #[state]
    pub(crate) head: sov_modules_api::StateValue<Block, BcsCodec>,

    /// Used only by the RPC: This represents the head of the chain and is set in two distinct stages:
    /// 1. `end_slot_hook`: the pending head is populated with data from pending_transactions.
    /// 2. `finalize_hook` the `root_hash` is populated.
    /// Since this value is not authenticated, it can be modified in the `finalize_hook` with the correct `state_root`.
    #[state]
    pub(crate) pending_head: sov_modules_api::AccessoryStateValue<Block, BcsCodec>,

    /// Used only by the RPC: The vec is extended with `pending_head` in `finalize_hook`.
    #[state]
    pub(crate) blocks: sov_modules_api::AccessoryStateVec<SealedBlock, BcsCodec>,

    /// Used only by the RPC: block_hash => block_number mapping.
    #[state]
    pub(crate) block_hashes: sov_modules_api::AccessoryStateMap<B256, u64, BcsCodec>,

    /// Used only by the RPC: List of processed transactions.
    #[state]
    pub(crate) transactions:
        sov_modules_api::AccessoryStateVec<TransactionSignedAndRecovered, BcsCodec>,

    /// Used only by the RPC: transaction_hash => transaction_index mapping.
    #[state]
    pub(crate) transaction_hashes: sov_modules_api::AccessoryStateMap<B256, u64, BcsCodec>,

    /// Used only by the RPC: Receipts.
    #[state]
    pub(crate) receipts: sov_modules_api::AccessoryStateVec<Receipt, BcsCodec>,

    #[phantom]
    phantom: core::marker::PhantomData<S>,
}

impl<S: sov_modules_api::Spec> sov_modules_api::Module for Evm<S> {
    type Spec = S;

    type Config = EvmConfig;

    type CallMessage = CallMessage;

    type Event = Event;

    fn genesis(
        &self,
        config: &Self::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<(), Error> {
        Ok(self.init_module(config, state)?)
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> Result<sov_modules_api::CallResponse, Error> {
        Ok(self.execute_call(msg, context, state)?)
    }
}

impl<S: sov_modules_api::Spec> Evm<S> {
    /// Get a EvmDb instance for the supplied state.
    pub fn get_db<'a, Ws: StateAccessor>(
        &self,
        state: &'a mut Ws,
    ) -> EvmDb<UnmeteredStateWrapper<'a, Ws>> {
        let infallible_state_accessor = state.to_unmetered();
        EvmDb::new(
            self.accounts.clone(),
            self.code.clone(),
            infallible_state_accessor,
        )
    }

    /// Access the Ethereum transaction receipts.
    pub fn receipts<Accessor: AccessoryStateReaderAndWriter>(
        &self,
        state: &mut Accessor,
    ) -> Vec<Receipt> {
        self.receipts.collect_infallible(state)
    }

    /// Access the Ethereum transactions.
    pub fn transactions<Accessor: AccessoryStateReaderAndWriter>(
        &self,
        state: &mut Accessor,
    ) -> Vec<TransactionSignedAndRecovered> {
        self.transactions.collect_infallible(state)
    }

    /// Access the Ethereum blocks.
    pub fn blocks<Accessor: AccessoryStateReaderAndWriter>(
        &self,
        state: &mut Accessor,
    ) -> Vec<SealedBlock> {
        self.blocks.collect_infallible(state)
    }

    /// Lookup an Ethereum account by address.
    pub fn get_account<Accessor: StateReader<User>>(
        &self,
        address: &Address,
        state: &mut Accessor,
    ) -> Result<Option<DbAccount>, Accessor::Error> {
        self.accounts.get(address, state)
    }

    /// Get the currently pending head block.
    pub fn pending_head<Accessor: AccessoryStateReader>(
        &self,
        state: &mut Accessor,
    ) -> Option<Block> {
        self.pending_head.get(state).unwrap_infallible()
    }

    /// Get the current head block.
    pub fn head<Accessor: StateReader<User>>(
        &self,
        state: &mut Accessor,
    ) -> Result<Option<Block>, Accessor::Error> {
        self.head.get(state)
    }

    /// Get the current block env.
    pub fn block_env<Accessor: StateReader<User>>(
        &self,
        state: &mut Accessor,
    ) -> Result<Option<BlockEnv>, Accessor::Error> {
        self.block_env.get(state)
    }

    /// Get the Evm chain config.
    pub fn cfg<Accessor: StateReader<User>>(
        &self,
        state: &mut Accessor,
    ) -> Result<Option<EvmChainConfig>, Accessor::Error> {
        self.cfg.get(state)
    }

    /// Access the pending Ethereum transactions.
    pub fn pending_transactions<Accessor: InfallibleStateReaderAndWriter<User>>(
        &self,
        state: &mut Accessor,
    ) -> Vec<PendingTransaction> {
        self.pending_transactions.collect_infallible(state)
    }

    /// Lookup the height of an Ethereum block based on the supplied hash.
    pub fn get_block_height_by_hash<Accessor: AccessoryStateReader>(
        &self,
        block_hash: &B256,
        state: &mut Accessor,
    ) -> Option<u64> {
        self.block_hashes.get(block_hash, state).unwrap_infallible()
    }

    /// Lookup the index of a Ethereum transaction based on the supplied hash.
    pub fn get_tx_index_by_hash<Accessor: AccessoryStateReader>(
        &self,
        tx_hash: &B256,
        state: &mut Accessor,
    ) -> Option<u64> {
        self.transaction_hashes
            .get(tx_hash, state)
            .unwrap_infallible()
    }
}
