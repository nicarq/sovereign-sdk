use std::fmt::Debug;

use anyhow::Result;
use schemars::JsonSchema;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{Context, EventEmitter, Spec, TxState};
use strum::{EnumDiscriminants, EnumIs, VariantArray};
use thiserror::Error;

use super::ValueSetter;
use crate::event::Event;

/// This enumeration represents the available call messages for interacting with the `sov-value-setter` module.
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Eq,
    Clone,
    JsonSchema,
    EnumDiscriminants,
    EnumIs,
    UniversalWallet,
)]
#[serde(rename_all = "snake_case")]
#[strum_discriminants(derive(VariantArray, EnumIs))]
pub enum CallMessage {
    /// Single value to set.
    SetValue(
        /// Singe new value.
        u32,
    ),
    /// Many values to set.
    SetManyValues(
        /// Many new values.
        Vec<u8>,
    ),
}

/// Example of a custom error.
#[derive(Debug, Error)]
pub enum SetValueError {
    /// Value tried to be set by a user that wasn't admin.
    #[error("Only admin can change the value")]
    WrongSender,
}

impl<S: Spec> ValueSetter<S> {
    /// Sets `value` field to the `new_value`, only admin is authorized to call this method.
    pub(crate) fn set_value(
        &self,
        new_value: u32,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        // If admin is not then early return:
        let admin = self.admin.get_or_err(state)??;

        if &admin != context.sender() {
            // Here we use a custom error type.
            Err(SetValueError::WrongSender)?;
        }

        // This is how we set a new value:
        self.value.set(&new_value, state)?;

        self.emit_event(state, Event::NewValue(new_value));

        Ok(())
    }

    pub(crate) fn set_values(
        &self,
        new_value: Vec<u8>,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        let admin = self.admin.get_or_err(state)??;

        if &admin != context.sender() {
            // Here we use a custom error type.
            Err(SetValueError::WrongSender)?;
        }

        // This is how we set a new value:
        self.many_values.set_all(new_value, state)?;
        Ok(())
    }
}
