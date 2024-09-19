use std::fmt::Debug;

use anyhow::Result;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{CallResponse, Context, EventEmitter, Spec, TxState};

use crate::event::Event;
use crate::ExampleModule;

/// This enumeration represents the available call messages for interacting with
/// the `ExampleModule` module.
/// The `derive` for [`schemars::JsonSchema`] is a requirement of
/// [`sov_modules_api::ModuleCallJsonSchema`].
#[cfg_attr(feature = "native", derive(schemars::JsonSchema))]
#[cfg_attr(
    feature = "arbitrary",
    derive(arbitrary::Arbitrary, proptest_derive::Arbitrary)
)]
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    Debug,
    PartialEq,
    UniversalWallet,
    Clone,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CallMessage {
    SetValue(u32),
}

impl<S: Spec> ExampleModule<S> {
    /// Sets `value` field to the `new_value`
    pub(crate) fn set_value(
        &self,
        new_value: u32,
        _context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse> {
        self.value.set(&new_value, state)?;
        self.emit_event(state, Event::Set { value: new_value });

        Ok(CallResponse::default())
    }
}
