//! Traits for the hyperlane protocol.
use sov_bank::Amount;
use sov_modules_api::{HexString, Spec, TxState};

use crate::types::HookType;

/// Allows a module to be used as a post-dispatch hook.
pub trait PostDispatchHook<S: Spec> {
    /// Get the hook type for a given address. Used by the relayer to determine which metadata to include with its message.
    fn hook_type(&self, addr: &S::Address, state: &mut impl TxState<S>)
        -> anyhow::Result<HookType>;

    /// Check if the hook supports metadata.
    fn supports_metadata(
        &self,
        metadata: &HexString,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<bool>;

    /// Post-dispatch hook. Called by the mailbox at the end of the `dispatch` method.
    fn post_dispatch(
        &mut self,
        metadata: &HexString,
        message: &HexString,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()>;

    /// Estimate the cost of dispatch, in the native currency of the chain.
    fn quote_dispatch(
        &self,
        metadata: &HexString,
        message: &HexString,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<Amount>;
}

/// A post-dispatch hook that does nothing.
pub enum NoOpPostDispatchHook {}

impl<S: Spec> PostDispatchHook<S> for NoOpPostDispatchHook {
    fn hook_type(
        &self,
        _addr: &S::Address,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<HookType> {
        Ok(HookType::Unused)
    }

    fn supports_metadata(
        &self,
        _metadata: &HexString,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn post_dispatch(
        &mut self,
        _metadata: &HexString,
        _message: &HexString,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
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
