use std::fmt::Debug;

use anyhow::Result;
use schemars::JsonSchema;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{Context, EventEmitter, Gas, Spec, TxState};
use strum::{EnumDiscriminants, EnumIs, VariantArray};
use thiserror::Error;

use super::TestModule;
use crate::event::Event;

/// This enumeration represents the available call messages for interacting with the `sov-test-module` module.
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
pub enum CallMessage<S: Spec> {
    /// Single value to set.
    SetValue {
        /// Singe new value.
        value: u32,
        /// Gas to charge. Don't charge gas if None.
        gas: Option<S::Gas>,
    },
    /// Many values to set.
    SetManyValues(
        /// Many new values.
        Vec<u8>,
    ),
    /// Assert the visible slot number is as expected.
    AssertVisibleSlotNumber {
        /// The expected visible slot number.
        expected_visible_slot_number: u64,
    },
    /// Assert the state root matches the expected value.
    AssertStateRoot {
        /// The expected state root.
        expected_state_root: Vec<u8>,
    },
}

/// Example of a custom error.
#[derive(Debug, Error)]
pub enum SetValueError<S: Spec> {
    /// Value tried to be set by a user that wasn't admin.
    #[error(
        "Only admin can change the value. The expected admin is {admin}, but the sender is {sender}"
    )]
    WrongSender {
        /// The expected admin.
        admin: S::Address,
        /// The sender.
        sender: S::Address,
    },
}

impl<S: Spec> TestModule<S> {
    /// Sets `value` field to the `new_value`, only admin is authorized to call this method.
    pub(crate) fn set_value(
        &self,
        new_value: u32,
        gas: Option<S::Gas>,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        let gas = gas.unwrap_or(<S::Gas as Gas>::zero());
        state.charge_gas(&gas)?;
        // If admin is not then early return:
        let admin = self.admin.get_or_err(state)??;

        if &admin != context.sender() {
            // Here we use a custom error type.
            Err(SetValueError::WrongSender::<S> {
                admin,
                sender: context.sender().clone(),
            })?;
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
            Err(SetValueError::WrongSender::<S> {
                admin,
                sender: context.sender().clone(),
            })?;
        }

        // This is how we set a new value:
        self.many_values.set_all(new_value, state)?;
        Ok(())
    }

    pub(crate) fn assert_visible_slot_number(
        &self,
        expected_visible_slot_number: u64,
        _context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        let visible_height = state.visible_slot_number_to_access();
        anyhow::ensure!(
            visible_height.get() == expected_visible_slot_number,
            "Visible height is not as expected. Expected {}, but got {}",
            expected_visible_slot_number,
            visible_height.get()
        );
        Ok(())
    }

    /// Assert the state root is as expected.
    pub(crate) fn assert_state_root(
        &self,
        expected_state_root: Vec<u8>,
        _context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        let state_root = self.latest_state_root.get_or_err(state)??;
        anyhow::ensure!(
            expected_state_root == state_root.as_ref(),
            "State root is not as expected. Expected {}, but got {}",
            hex::encode(expected_state_root),
            state_root
        );
        Ok(())
    }
}
