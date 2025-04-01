use borsh::{BorshDeserialize, BorshSerialize};
use sov_bank::Amount;
use sov_modules_api::{
    BorrowedMut, Context, DaSpec, Error, EventEmitter, GenesisState, HexHash, HexString, Module,
    ModuleId, ModuleInfo, ModuleRestApi, Spec, StateValue, TxState,
};
mod tree;
use serde::{Deserialize, Serialize};
use tree::MerkleTree;

use crate::traits::PostDispatchHook;
use crate::types::{keccak256_hash, HookType};
/// A helper modules which merklizes each message as it is dispatched.
// Note: In a future iteration, we may consider moving this into the Mailbox module.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct MerkleTreeHooks<S: Spec> {
    /// Module identifier.
    #[id]
    pub id: ModuleId,

    /// Owners of the hooks.
    #[state]
    pub tree: StateValue<MerkleTree>,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

impl<S: Spec> Module for MerkleTreeHooks<S> {
    type Spec = S;
    type Config = ();
    type CallMessage = ();
    type Event = Event;

    fn genesis(
        &mut self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        _config: &Self::Config,
        _state: &mut impl GenesisState<S>,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn call(
        &mut self,
        _msg: Self::CallMessage,
        _context: &Context<S>,
        _state: &mut impl TxState<S>,
    ) -> Result<(), Error> {
        Ok(())
    }
}
/// Events that can be emitted by the Merkle module.
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// Emitted when a new element is inserted into the tree.
    InsertedIntoTree {
        /// The hash of the inserted element.
        id: HexHash,
        /// The index of the inserted element.
        index: u32,
    },
}

/// Implementation of `PostDispatchHook` for the Merkle tree hooks module.
impl<S: Spec> PostDispatchHook<S> for MerkleTreeHooks<S> {
    fn hook_type(
        &self,
        _addr: &S::Address,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<HookType> {
        Ok(HookType::MerkleTree)
    }

    fn supports_metadata(
        &self,
        _metadata: &HexString,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }

    // compare to https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/ff0d4af74ecc586ef0c036e37fa4cf9c2ba5050e/solidity/contracts/hooks/MerkleTreeHook.sol#L63
    fn post_dispatch(
        &mut self,
        _metadata: &HexString,
        message: &HexString,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        let id = keccak256_hash(&message.0);
        let mut tree: BorrowedMut<MerkleTree, _> =
            self.tree.borrow_mut(state)?.unwrap_or(Default::default());
        let index = tree.count;

        tree.insert(id)?;
        tree.save(state)?;

        self.emit_event(state, Event::InsertedIntoTree { index, id });

        Ok(())
    }

    fn quote_dispatch(
        &self,
        _metadata: &HexString,
        _message: &HexString,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<Amount> {
        Ok(Amount::ZERO)
    }
}
