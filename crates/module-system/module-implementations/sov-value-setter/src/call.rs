use std::fmt::Debug;

use anyhow::Result;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use schemars::JsonSchema;
use sov_modules_api::digest::Digest;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{Context, CryptoSpec, EventEmitter, Gas, Spec, TxState};
use strum::{EnumDiscriminants, EnumIs, VariantArray};
use thiserror::Error;

use super::{ValueSetter, VERY_LARGE_VEC_LENGTH};
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
#[schemars(bound = "S::Gas: ::schemars::JsonSchema", rename = "CallMessage")]
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
    /// Read and set many individual values.
    ReadAndSetManyIndividualValues {
        /// The number of values to read and set.
        number_of_operations: u64,
        /// The salt.
        salt: u64,
    },
    /// Read and set entries in a large vector stored as a `StateValue`
    ReadAndSetHeavyState {
        /// The number of new values to read and set.
        number_of_new_values: u64,
        /// The max size of the heavy state.
        max_heavy_state_size: u64,
        /// The salt.
        salt: u64,
    },
    /// Run CPU heavy operation. Each iteration computes a hash with the Spec::Hasher.
    RunCPUHeavyOperation {
        /// The number of iterations.
        iterations: u64,
    },
    /// Trigger a panic.
    Panic,
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

impl<S: Spec> ValueSetter<S> {
    /// Sets `value` field to the `new_value`, only admin is authorized to call this method.
    pub(crate) fn set_value(
        &mut self,
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
        &mut self,
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

    pub(crate) fn read_and_set_many_individual_values(
        &mut self,
        number_of_operations: u64,
        salt: u64,
        _context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        // Use a ChaCha8-based PRNG seeded with the supplied salt.
        let mut rng = ChaCha8Rng::seed_from_u64(salt);

        for _ in 0..number_of_operations {
            // Generate pseudo-random indices for read and write operations.
            let read_index: u64 = rng.gen_range(0..VERY_LARGE_VEC_LENGTH);
            let write_index: u64 = rng.gen_range(0..VERY_LARGE_VEC_LENGTH);

            // Read from the random location.
            let value_read = self.very_large_vec.get(read_index, state)?.unwrap_or(0);
            let value_to_write = value_read.wrapping_add(1);

            // Write to the random location
            let _ = self
                .very_large_vec
                .set(write_index, &value_to_write, state)?;
        }

        self.emit_event(
            state,
            Event::ReadAndSetManyIndividualValues(number_of_operations),
        );
        Ok(())
    }

    pub(crate) fn read_and_set_heavy_state(
        &mut self,
        number_of_new_values: u64,
        max_heavy_state_size: u64,
        salt: u64,
        _context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        // Read the entire huge vector from state.
        let mut old_vec: Vec<u64> = self.heavy_state.get(state)?.unwrap_or_default();

        // Modify the vector by appending new values.
        let new_values = vec![salt; number_of_new_values as usize];
        old_vec.extend(new_values);

        // Truncate if the vector is over the size limit.
        // This removes the oldest data (from the front) to make room for the new data.
        if old_vec.len() > max_heavy_state_size as usize {
            let num_to_remove = old_vec.len() - max_heavy_state_size as usize;
            old_vec.drain(0..num_to_remove);
        }

        // Write the entire modified vector back to state.
        self.heavy_state.set::<Vec<u64>, _>(&old_vec, state)?;

        self.emit_event(
            state,
            Event::ReadAndSetHeavyState(number_of_new_values, salt),
        );

        Ok(())
    }

    pub(crate) fn run_cpu_heavy_operation(
        &mut self,
        iterations: u64,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        let mut current_hash =
            <S::CryptoSpec as CryptoSpec>::Hasher::digest(context.sender().as_ref());
        for _ in 0..iterations {
            current_hash = <S::CryptoSpec as CryptoSpec>::Hasher::digest(&current_hash[..]);
        }
        self.emit_event(
            state,
            Event::RanCPUHeavyOperation(iterations, current_hash.to_vec()),
        );
        Ok(())
    }

    pub(crate) fn assert_visible_slot_number(
        &self,
        expected_visible_slot_number: u64,
        _context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        let visible_height = state.current_visible_slot_number();
        anyhow::ensure!(
            visible_height.get() == expected_visible_slot_number,
            "Visible height is not as expected. Expected {}, but got {}",
            expected_visible_slot_number,
            visible_height.get()
        );
        Ok(())
    }
}
