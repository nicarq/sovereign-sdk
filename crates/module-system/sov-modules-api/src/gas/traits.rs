//! Gas unit definitions and implementations.

use core::fmt::{self, Debug, Display};
use std::cmp::min;

use anyhow::Result;
use borsh::{BorshDeserialize, BorshSerialize};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_modules_macros::config_value_private;
use sov_universal_wallet::schema::{Container, IndexLinking, Item, Link, Schema, SchemaGenerator};
use sov_universal_wallet::ty::{Tuple, UnnamedField};
use thiserror::Error;

use crate::{DaSpec, Spec};

const GAS_DIMENSIONS: usize = config_value_private!(
    "GAS_DIMENSIONS",
    "Couldn't parse `GAS_DIMENSIONS` in TOML file; must be a constant integer (e.g. `GAS_DIMENSIONS = { const = 2 }`)"
);

/// A multi-dimensional gas unit represented as an array of `u64`.`
pub trait GasArray:
    'static
    + fmt::Debug
    + Display
    + Clone
    + Send
    + Sync
    + PartialEq
    + Eq
    + JsonSchema
    + core::hash::Hash
    + Serialize
    + DeserializeOwned
    + BorshSerialize
    + BorshDeserialize
    + SchemaGenerator
    + From<[u64; GAS_DIMENSIONS]>
    + Into<[u64; GAS_DIMENSIONS]>
    + AsRef<[u64; GAS_DIMENSIONS]>
    + AsMut<[u64; GAS_DIMENSIONS]>
    + TryFrom<Vec<u64>, Error: Into<anyhow::Error> + Debug>
{
    /// A zeroed instance of the unit.
    const ZEROED: Self;

    /// The maximum value of the gas unit.
    const MAX: Self;

    /// Returns the sum of the two gas units or None if the result overflows.
    fn checked_combine(&self, rhs: &Self) -> Option<Self>;

    /// Out-of-place substraction of gas units.
    ///
    /// # Output
    /// Returns [`None`] if the substraction in any gas dimension underflows.
    fn checked_sub(&self, rhs: &Self) -> Option<Self>;

    /// Returns the product of the scalar and the gas units or None if the result overflows.
    fn checked_scalar_product(&self, scalar: u64) -> Option<Self>;

    /// Checks if the gas is less than the provided gas in each dimension of the gas array.
    fn dim_is_less_than(&self, rhs: &Self) -> bool;

    /// Checks if the gas is less or equal to the provided gas in each dimension of the gas array.
    fn dim_is_less_or_eq(&self, rhs: &Self) -> bool;

    /// Calculates the minimum gas values between two gas arrays along each dimension.
    fn calculate_min(lhs: &Self, rhs: &Self) -> Self;

    /// In-place division of gas units.
    fn scalar_division(&mut self, scalar: u64) -> &mut Self;

    #[cfg(feature = "test-utils")]
    /// In-place addition of gas units with a scalar.
    fn scalar_add(&mut self, scalar: u64) -> &mut Self;

    #[cfg(feature = "test-utils")]
    /// In-place substraction of gas units with a scalar.
    fn scalar_sub(&mut self, scalar: u64) -> &mut Self;
}

/// A unit of gas
pub trait Gas: GasArray {
    /// The price of the gas, expressed in tokens per unit.
    type Price: GasArray;

    /// Calculates the value of the given amount of gas at the given price or returns None if the result overflows.
    fn checked_value(&self, price: &Self::Price) -> Option<u64>;

    /// Calculates the value of the given amount of gas at the given price.
    fn value(&self, price: &Self::Price) -> u64;

    /// Returns a gas unit which is zero in all dimensions.
    fn zero() -> Self {
        Self::ZEROED
    }

    /// Returns the maximum gas unit.
    fn max() -> Self {
        Self::MAX
    }

    #[cfg(feature = "gas-constant-estimation")]
    /// Returns an optional name of the gas unit.
    fn name(&self) -> &Option<String>;

    #[cfg(feature = "gas-constant-estimation")]
    /// Names the gas unit.
    fn with_name(self, name: String) -> Self;
}

/// A multi-dimensional gas unit.
#[derive(Clone, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, derive_more::Display)]
#[display("GasUnit{:?}", self.value)]
pub struct GasUnit<const N: usize> {
    value: [u64; N],
    #[cfg(feature = "gas-constant-estimation")]
    #[borsh(skip)]
    name: Option<String>,
}

impl<const N: usize> SchemaGenerator for GasUnit<N>
where
    Self: 'static,
    [u64; N]: SchemaGenerator,
{
    fn scaffold() -> Item<IndexLinking> {
        Item::Container(Container::Tuple(Tuple {
            template: None,
            fields: vec![UnnamedField {
                value: Link::Placeholder,
                silent: false,
                doc: String::new(),
            }],
        }))
    }

    fn get_child_links(schema: &mut Schema) -> Vec<Link> {
        vec![<[u64; N]>::make_linkable(schema)]
    }
}

impl<const N: usize> Debug for GasUnit<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

/// A gas price for multi-dimensional gas.
#[derive(
    Clone,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    sov_rollup_interface::sov_universal_wallet::UniversalWallet,
    derive_more::Display,
)]
#[sov_wallet()]
#[display("GasPrice{:?}", self.value)]
pub struct GasPrice<const N: usize> {
    value: [u64; N],
}

impl<const N: usize> Debug for GasPrice<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

macro_rules! impl_gas_dimensions {
    ($t: ty, $t_name: literal, $n: expr) => {
        impl schemars::JsonSchema for $t {
            fn schema_name() -> String {
                $t_name.to_owned() + "(" + &format!("{}", $n) + ")"
            }

            fn json_schema(_gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
                serde_json::from_value(serde_json::json!({
                    "type": "array",
                    "minItems": $n,
                    "maxItems": $n,
                    "items": {
                        "type": "number"
                    },
                    // This description assumes that `serializer` uses a human-readable format.
                    "description": $t_name.to_owned() + " is an array of u64 of size " + &format!("{}", $n),
                }))
                .unwrap()
            }
        }

        impl ::serde::Serialize for $t {
            fn serialize<__S>(&self, serializer: __S) -> Result<__S::Ok, __S::Error>
            where
                __S: serde::Serializer,
            {
                <[u64; $n] as serde::Serialize>::serialize(&self.value, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $t {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let array = <[u64; $n] as serde::Deserialize>::deserialize(deserializer)?;
                Ok(Self::from(array))
            }
        }

        impl From<[u64; $n]> for $t {
            fn from(array: [u64; $n]) -> Self {
                Self::from_primitive(array)
            }
        }

        impl From<$t> for [u64; $n] {
            fn from(gas: $t) -> [u64; $n] {
                gas.value
            }
        }

        impl TryFrom<Vec<u64>> for $t {
            type Error = anyhow::Error;

            fn try_from(value: Vec<u64>) -> Result<Self, Self::Error> {
                if value.len() != $n {
                    anyhow::bail!("Impossible to convert to a gas unit. The array must have {} elements, but it has {}", $n, value.len());
                }

                let mut output = [0; $n];
                output.copy_from_slice(&value);

                Ok(Self::from(output))
            }
        }

        impl AsRef<[u64; $n]> for $t {
            fn as_ref(&self) -> &[u64; $n] {
                &self.value
            }
        }

        impl AsMut<[u64; $n]> for $t {
            fn as_mut(&mut self) -> &mut [u64; $n] {
                &mut self.value
            }
        }


        impl GasArray for $t {
            const ZEROED: Self = Self::from_primitive([0; $n]);

            const MAX: Self = Self::from_primitive([u64::MAX; $n]);

            fn checked_sub(&self, rhs: &Self) -> Option<Self> {
                let mut output = [0; $n];

                for (i, (l, r)) in self.value.iter().zip(rhs.value.as_slice()).enumerate() {
                    if let Some(res) = l.checked_sub(*r) {
                        output[i] = res;
                    } else {
                        return None
                    }
                }

                Some(Self::from(output))
            }

            fn checked_scalar_product(&self, scalar: u64) -> Option<Self> {
                let mut output = [0; $n];

                for (i,v) in self.value.iter().enumerate() {
                    if let Some(res) = v.checked_mul(scalar) {
                        output[i] = res;
                    }else {
                        return None
                    }
                }

                Some(Self::from(output))

            }

            fn dim_is_less_than(&self, rhs: &Self) -> bool{
                for (l, r) in self.value.iter().zip(rhs.value.as_slice()) {
                    if l >=r {
                        return false
                    }
                }
                true
            }

            fn dim_is_less_or_eq(&self, rhs: &Self) -> bool{
                for (l, r) in self.value.iter().zip(rhs.value.as_slice()) {
                    if l > r{
                        return false
                    }
                }
                true
            }

            fn calculate_min(lhs: &Self, rhs: &Self) -> Self{
                let mut output = [0; $n];

                for (i, (l,r)) in lhs.value.iter().zip(rhs.value.iter()).enumerate() {
                    output[i] = min(*l, *r);
                }
                Self::from_primitive(output)
            }

            fn scalar_division(&mut self, scalar: u64) -> &mut Self {
                self.value
                    .iter_mut()
                    .for_each(|s| *s = s.checked_div(scalar).unwrap_or(0));
                self
            }

            #[cfg(feature = "test-utils")]
            fn scalar_add(&mut self, scalar: u64) -> &mut Self {
                self.value
                    .iter_mut()
                    .for_each(|s| *s = s.saturating_add(scalar));
                self
            }

            #[cfg(feature = "test-utils")]
            fn scalar_sub(&mut self, scalar: u64) -> &mut Self {
                self.value
                    .iter_mut()
                    .for_each(|s| *s = s.saturating_sub(scalar));
                self
            }

            fn checked_combine(&self, rhs: &Self) -> Option<Self> {
                let mut output = [0; $n];

                for (i, (l, r)) in self.value.iter().zip(rhs.value.iter()).enumerate() {
                    if let Some(res) = l.checked_add(*r) {
                        output[i] = res;
                    } else {
                        return None
                    }
                }
                Some(Self::from_primitive(output))
            }
        }
    };
}

macro_rules! impl_gas_unit {
    ($n:expr) => {
        impl Gas for GasUnit<$n> {
            type Price = GasPrice<$n>;

            #[cfg(feature = "gas-constant-estimation")]
            fn name(&self) -> &Option<String> {
                &self.name
            }

            /// Adds a name tag to the gas constant.
            #[cfg(feature = "gas-constant-estimation")]
            fn with_name(self, name: String) -> Self {
                Self {
                    name: Some(name),
                    ..self
                }
            }

            fn checked_value(&self, price: &Self::Price) -> Option<u64> {
                let mut value: u64 = 0;
                for (g, p) in self.value.iter().zip(price.as_ref().iter().copied()) {
                    let v = g.checked_mul(p)?;
                    value = value.checked_add(v)?;
                }

                Some(value)
            }

            fn value(&self, price: &Self::Price) -> u64 {
                self.value
                    .iter()
                    .zip(price.as_ref().iter().copied())
                    .map(|(a, b)| a.saturating_mul(b))
                    .fold(0, |a, b| a.saturating_add(b))
            }
        }

        impl GasUnit<$n> {
            /// Creates a new [`GasUnit`] from an array of [`u64`].
            const fn from_primitive(array: [u64; $n]) -> Self {
                Self {
                    value: array,
                    #[cfg(feature = "gas-constant-estimation")]
                    name: None,
                }
            }
        }

        impl GasPrice<$n> {
            /// Creates a new [`GasPrice`] from an array of [`u64`].
            pub const fn from_primitive(array: [u64; $n]) -> Self {
                Self { value: array }
            }
        }

        impl_gas_dimensions!(GasUnit<$n>, "GasUnit", $n);
        impl_gas_dimensions!(GasPrice<$n>, "GasPrice", $n);
    };
}

impl_gas_unit!(GAS_DIMENSIONS);

/// Error type that can be raised by the `GasMeter` trait.
/// Errors can be raised either when the meter runs out of gas or when the refund operation fails.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GasMeteringError<GU: Gas> {
    #[error("Gas calculation overflow: {0}")]
    /// Unable to calculate gas usage due to overflow.
    Overflow(String),
    /// The slot gas limit has been exhausted.
    #[error("The slot gas limit has been exhausted")]
    SlotOutOfGas,
    /// Unable to deserialize data due to invalid length.
    #[error("Unable to deserialize data due to invalid length: {0}")]
    InvalidLength(String),
    /// The gas meter has ran out of gas.
    #[error("The gas to charge is greater than the funds available in the meter. Gas to charge {gas_to_charge}, gas price {gas_price}, initial_gas {initial_gas}")]
    OutOfGas {
        /// The amount of gas to charge.
        gas_to_charge: GU,
        /// The current gas price.
        gas_price: GU::Price,
        /// The initial gas.
        initial_gas: GU,
    },
    /// The refund operation failed for the gas meter.
    #[error("The gas to refund is greater than the gas used. Gas to refund {gas_to_refund}, gas used {gas_used}")]
    ImpossibleToRefundGas {
        /// Amount of gas to refund.
        gas_to_refund: GU,
        /// Amount of gas currently used by the meter.
        gas_used: GU,
    },
}

/// Contain information about the gas usage of a gas.
pub struct GasInfo<GU: Gas> {
    /// The gas value.
    pub gas_value: u64,
    /// The current gas used accumulated by the stake meter.
    pub gas_used: GU,
    /// The current gas price
    pub gas_price: GU::Price,
    /// The remaining amount of tokens locked in the meter
    pub remaining_funds: u64,
}

#[macro_export]
/// Defines a constant gas value.
macro_rules! new_constant {
    ($name: literal, $gas: ty) => {{
        #[cfg(feature = "gas-constant-estimation")]
        {
            <$gas>::from(config_value_private!($name)).with_name($name.to_string())
        }
        #[cfg(not(feature = "gas-constant-estimation"))]
        {
            <$gas>::from(config_value_private!($name))
        }
    }};
}

/// A type-safe trait that should track the gas consumed by a finite ressource over time.
pub trait GasMeter {
    /// The spec used by this gas meter.
    type Spec: Spec;

    /// Charges a fix amount of gas in the gas meter.
    ///
    /// # Error
    /// May raises an error if the gas to charge is greater than the funds available
    fn charge_gas(
        &mut self,
        amount: &<Self::Spec as Spec>::Gas,
    ) -> Result<(), GasMeteringError<<Self::Spec as Spec>::Gas>>;

    /// Charges an amount of gas equal to `amount *_point parameter`, the pointwise product of `amount` times `parameter`.
    fn charge_linear_gas(
        &mut self,
        amount: &<Self::Spec as Spec>::Gas,
        parameter: u64,
    ) -> Result<(), GasMeteringError<<Self::Spec as Spec>::Gas>>;

    /// Refunds some gas to the gas meter.
    ///
    /// ## Note
    /// This method may fail if the gas to refund is greater than the funds charged to the gas meter.
    /// In that case, the gas meter won't be updated and the refund will fail.
    fn refund_gas(
        &mut self,
        gas: &<Self::Spec as Spec>::Gas,
    ) -> Result<(), GasMeteringError<<Self::Spec as Spec>::Gas>>;

    /// Returns gas usage.
    fn gas_info(&self) -> GasInfo<<Self::Spec as Spec>::Gas>;
}

/// A gas meter that tracks the gas used for a slot.
pub struct SlotGasMeter<S: Spec> {
    preferred_sequencer: Option<<S::Da as DaSpec>::Address>,
    initial_slot_gas: S::Gas,
    // Assumption: The preferred batches/proofs are executed before standard batches/proofs.
    remaining_preferred_slot_gas: S::Gas,
    remaining_total_slot_gas: S::Gas,
}

impl<S: Spec> SlotGasMeter<S> {
    /// Creates a new `SlotGasMeter`
    pub fn new(
        remaining_slot_gas: S::Gas,
        preferred_sequencer: Option<<S::Da as DaSpec>::Address>,
    ) -> Self {
        // remaining_preferred_slot_gas = 0.9 * remaining_slot_gas
        let remaining_preferred_slot_gas = remaining_slot_gas
            .clone()
            .scalar_division(10)
            .checked_scalar_product(9)
            .unwrap();

        Self {
            preferred_sequencer,
            remaining_preferred_slot_gas,
            initial_slot_gas: remaining_slot_gas.clone(),
            remaining_total_slot_gas: remaining_slot_gas,
        }
    }

    /// Get the remaining slot gas.
    pub fn remaining_slot_gas(&self, sequencer: &<S::Da as DaSpec>::Address) -> &S::Gas {
        if Some(sequencer) == self.preferred_sequencer.as_ref() {
            &self.remaining_preferred_slot_gas
        } else {
            &self.remaining_total_slot_gas
        }
    }

    /// Charge gas.
    pub fn charge_gas(
        &mut self,
        gas: &S::Gas,
        sequencer: &<S::Da as DaSpec>::Address,
    ) -> Result<(), GasMeteringError<S::Gas>> {
        // Preferred transactions reduce both the "preferred" gas and the "total" gas,
        // while standard transactions only reduce the `total` gas.
        // The `initial total` gas is always greater than the "initial preferred" gas, and preferred
        // transactions are executed before standard transactions.
        // This mechanism ensures that the preferred sequencer cannot fully censor
        // other sequencers (or emergency registrations) by exhausting all the available gas.
        if Some(sequencer) == self.preferred_sequencer.as_ref() {
            self.remaining_preferred_slot_gas = self
                .remaining_preferred_slot_gas
                .checked_sub(gas)
                .ok_or(GasMeteringError::SlotOutOfGas)?;
        }

        self.remaining_total_slot_gas = self
            .remaining_total_slot_gas
            .checked_sub(gas)
            .ok_or(GasMeteringError::SlotOutOfGas)?;

        Ok(())
    }

    /// Total gas used in the slot.
    pub fn total_gas_used(&self) -> S::Gas {
        self.initial_slot_gas
            .checked_sub(&self.remaining_total_slot_gas)
            .expect("The remaining_slot_gas can't be greater than the initial_slot_gas")
    }
}

/// A struct that keeps track of the gas used.
/// The gas meter continues running until it either depletes its funds or runs out of gas, depending on its configuration.
/// It also ensures that the gas used will not overflow when multiplied by the gas price.
#[derive(Clone, Debug)]
pub struct BasicGasMeter<S: Spec> {
    initial_gas: S::Gas,
    remaining_gas: S::Gas,
    remaining_funds: Option<u64>,
    gas_price: <S::Gas as Gas>::Price,
}

impl<S: Spec> BasicGasMeter<S> {
    /// Creates a new `BasicGasMeter`.
    pub fn new_with_funds_and_gas(
        remaining_funds: u64,
        remaining_gas: S::Gas,
        gas_price: <S::Gas as Gas>::Price,
    ) -> Self {
        Self {
            initial_gas: remaining_gas.clone(),
            remaining_gas,
            remaining_funds: Some(remaining_funds),
            gas_price,
        }
    }

    /// Creates a new `BasicGasMeter`
    pub fn new_with_gas(remaining_gas: S::Gas, gas_price: <S::Gas as Gas>::Price) -> Self {
        Self {
            initial_gas: remaining_gas.clone(),
            remaining_gas,
            remaining_funds: None,
            gas_price,
        }
    }

    /// Replaces the gas price in the provided meter.
    #[cfg(feature = "native")]
    pub(crate) fn set_gas_price(&mut self, gas_price: <S::Gas as Gas>::Price) {
        self.gas_price = gas_price;
    }

    fn compute_remaining_funds(
        &self,
        remaining_funds: &u64,
        amount: &S::Gas,
    ) -> Result<u64, GasMeteringError<S::Gas>> {
        let amount_value = amount.checked_value(&self.gas_price).ok_or_else(|| {
            GasMeteringError::Overflow(
                "Charge Funds: Unable to charge gas, because the calculation overflows".to_string(),
            )
        })?;

        remaining_funds
            .checked_sub(amount_value)
            .ok_or_else(|| GasMeteringError::OutOfGas {
                gas_to_charge: amount.clone(),
                gas_price: self.gas_price.clone(),
                initial_gas: self.initial_gas.clone(),
            })
    }

    fn compute_remaining_gas(
        &self,
        remaining_gas: &S::Gas,
        amount: &S::Gas,
    ) -> Result<S::Gas, GasMeteringError<S::Gas>> {
        remaining_gas
            .checked_sub(amount)
            .ok_or_else(|| GasMeteringError::OutOfGas {
                gas_to_charge: amount.clone(),
                gas_price: self.gas_price.clone(),
                initial_gas: self.initial_gas.clone(),
            })
    }

    fn charge_gas_inner(&mut self, amount: &S::Gas) -> Result<(), GasMeteringError<S::Gas>> {
        if let Some(remaining_funds) = &self.remaining_funds {
            self.remaining_funds = Some(self.compute_remaining_funds(remaining_funds, amount)?);
        }

        let remaining_gas = self.compute_remaining_gas(&self.remaining_gas, amount)?;
        // Here we check that the current gas_used won't overflow when multiplied by the price.
        // This ensures that after execution, it is always safe to convert the total gas used to a token value.
        {
            let gas_used = self
                .initial_gas
                .checked_sub(&remaining_gas)
                .expect("The remaining gas can't be greater than the initial gas");

            gas_used.checked_value(&self.gas_price).ok_or_else(|| {
                GasMeteringError::Overflow(
                    "Charge Gas: Unable to charge gas, because the calculation overflows"
                        .to_string(),
                )
            })?;
        }
        self.remaining_gas = remaining_gas;

        Ok(())
    }
}

impl<S: Spec> GasMeter for BasicGasMeter<S> {
    type Spec = S;
    fn charge_gas(&mut self, amount: &S::Gas) -> Result<(), GasMeteringError<S::Gas>> {
        self.charge_gas_inner(amount)?;

        #[cfg(all(feature = "gas-constant-estimation", feature = "native"))]
        if let Some(name) = amount.name() {
            sov_metrics::GAS_CONSTANTS.with(|var| {
                let mut var = var.borrow_mut();

                if let Some(const_count) = var.get_mut(name) {
                    *const_count = const_count.checked_add(1).unwrap();
                } else {
                    var.insert(name.clone(), 1);
                }
            });
        }

        Ok(())
    }

    fn charge_linear_gas(
        &mut self,
        amount: &S::Gas,
        parameter: u64,
    ) -> Result<(), GasMeteringError<<Self::Spec as Spec>::Gas>> {
        self.charge_gas_inner(&amount.checked_scalar_product(parameter).ok_or(
            GasMeteringError::Overflow(format!(
                "Unable to charge gas. The product of {} to {} is overflowing",
                amount, parameter
            )),
        )?)?;

        #[cfg(all(feature = "gas-constant-estimation", feature = "native"))]
        if let Some(name) = amount.name() {
            sov_metrics::GAS_CONSTANTS.with(|var| {
                let param_i64 = parameter.try_into().unwrap();

                let mut var = var.borrow_mut();

                if let Some(const_count) = var.get_mut(name) {
                    *const_count = const_count.checked_add(param_i64).unwrap();
                } else {
                    var.insert(name.clone(), param_i64);
                }
            });
        }

        Ok(())
    }

    fn refund_gas(&mut self, gas: &S::Gas) -> Result<(), GasMeteringError<S::Gas>> {
        // `refund_gas` is called in accessors to refund gas for hot access/write/delete operations.
        // It is always preceded by `charge_gas`, and the refund amount is less than the charged amount.
        // Although overflows are handled, they should not occur under normal circumstances.
        {
            let gas_used = self
                .initial_gas
                .checked_sub(&self.remaining_gas)
                .expect("The remaining gas can't be greater than the initial gas");

            if gas_used.dim_is_less_than(gas) {
                return Err(GasMeteringError::ImpossibleToRefundGas {
                    gas_to_refund: gas.clone(),
                    gas_used: gas_used.clone(),
                });
            }
        }

        let gas_value = gas.checked_value(&self.gas_price).ok_or_else(|| {
            GasMeteringError::Overflow(
                "Refund Gas: Unable to refund gas, because the calculation overflows".to_string(),
            )
        })?;

        if let Some(remaining_funds) = self.remaining_funds {
            // We never refund more than what was charged during execution; therefore, an overflow is impossible.
            self.remaining_funds =
                Some(remaining_funds.checked_add(gas_value).ok_or_else(|| {
                    GasMeteringError::Overflow("Refund Gas: remaining funds overflow".to_string())
                })?);
        }

        // We never refund more than what was charged during execution; therefore, an overflow is impossible.
        self.remaining_gas = self.remaining_gas.checked_combine(gas).ok_or_else(|| {
            GasMeteringError::Overflow("Refund Gas: remaining gas overflow".to_string())
        })?;

        #[cfg(all(feature = "gas-constant-estimation", feature = "native"))]
        if let Some(name) = gas.name() {
            sov_metrics::GAS_CONSTANTS.with(|var| {
                let mut var = var.borrow_mut();

                if let Some(const_count) = var.get_mut(name) {
                    *const_count = const_count.checked_sub(1).unwrap();
                } else {
                    var.insert(name.clone(), -1);
                }
            });
        }

        Ok(())
    }

    fn gas_info(&self) -> GasInfo<S::Gas> {
        let remaining_funds = if let Some(remaining_funds) = self.remaining_funds {
            remaining_funds
        } else {
            self.remaining_gas.value(&self.gas_price)
        };

        let gas_used = self
            .initial_gas
            .checked_sub(&self.remaining_gas)
            .expect("The remaining gas can't be greater than the initial gas");

        let gas_value = gas_used
            .checked_value(&self.gas_price)
            .expect("The gas value should be possible to compute");

        GasInfo {
            gas_value,
            gas_used,
            gas_price: self.gas_price.clone(),
            remaining_funds,
        }
    }
}

#[cfg(test)]
mod tests {
    use sov_mock_da::{MockAddress, MockDaSpec};
    use sov_mock_zkvm::MockZkvm;

    use super::*;
    use crate::default_spec::DefaultSpec;
    use crate::execution_mode::Native;

    type S = DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, Native>;

    #[test]
    fn is_less_than_test() {
        let gas_1 = GasUnit::<2>::from([10, 20]);
        let gas_2 = GasUnit::<2>::from([20, 30]);
        assert!(gas_1.dim_is_less_than(&gas_2));
        assert!(gas_1.dim_is_less_or_eq(&gas_2));

        let gas_1 = GasUnit::<2>::from([20, 30]);
        let gas_2 = GasUnit::<2>::from([20, 30]);
        assert!(gas_1.dim_is_less_or_eq(&gas_2));

        let gas_1 = GasUnit::<2>::from([10, 40]);
        let gas_2 = GasUnit::<2>::from([20, 30]);
        assert!(!gas_1.dim_is_less_than(&gas_2));
        assert!(!gas_1.dim_is_less_or_eq(&gas_2));

        let gas_1 = GasUnit::<2>::from([40, 40]);
        let gas_2 = GasUnit::<2>::from([20, 30]);
        assert!(!gas_1.dim_is_less_than(&gas_2));
        assert!(!gas_1.dim_is_less_or_eq(&gas_2));

        let gas_1 = GasUnit::<2>::from([40, 40]);
        let gas_2 = GasUnit::<2>::from([20, 50]);
        assert!(!gas_1.dim_is_less_than(&gas_2));
        assert!(!gas_1.dim_is_less_or_eq(&gas_2));

        let gas_1 = GasUnit::<2>::from([10, 20]);
        let gas_2 = GasUnit::<2>::from([10, 30]);
        assert!(!gas_1.dim_is_less_than(&gas_2));

        let gas_1 = GasUnit::<2>::from([10, 30]);
        let gas_2 = GasUnit::<2>::from([20, 30]);
        assert!(!gas_1.dim_is_less_than(&gas_2));

        let gas_1 = GasUnit::<2>::from([10, 20]);
        let gas_2 = GasUnit::<2>::from([10, 30]);
        assert!(gas_1.dim_is_less_or_eq(&gas_2));

        let gas_1 = GasUnit::<2>::from([10, 30]);
        let gas_2 = GasUnit::<2>::from([20, 30]);
        assert!(gas_1.dim_is_less_or_eq(&gas_2));
    }

    #[test]
    fn calculate_min_test() {
        let gas_1 = GasUnit::<2>::from([10, 20]);
        let gas_2 = GasUnit::<2>::from([20, 30]);

        assert_eq!(
            GasUnit::<2>::from([10, 20]),
            GasUnit::calculate_min(&gas_1, &gas_2)
        );

        let gas_1 = GasUnit::<2>::from([20, 30]);
        let gas_2 = GasUnit::<2>::from([10, 20]);

        assert_eq!(
            GasUnit::<2>::from([10, 20]),
            GasUnit::calculate_min(&gas_1, &gas_2)
        );

        let gas_1 = GasUnit::<2>::from([10, 20]);
        let gas_2 = GasUnit::<2>::from([10, 5]);

        assert_eq!(
            GasUnit::<2>::from([10, 5]),
            GasUnit::calculate_min(&gas_1, &gas_2)
        );

        let gas_1 = GasUnit::<2>::from([10, 20]);
        let gas_2 = GasUnit::<2>::from([5, 30]);

        assert_eq!(
            GasUnit::<2>::from([5, 20]),
            GasUnit::calculate_min(&gas_1, &gas_2)
        );

        let gas_1 = GasUnit::<2>::from([10, 20]);
        let gas_2 = GasUnit::<2>::from([10, 20]);

        assert_eq!(
            GasUnit::<2>::from([10, 20]),
            GasUnit::calculate_min(&gas_1, &gas_2)
        );
    }

    #[test]
    fn checked_scalar_product_test() {
        let gas = GasUnit::<2>::from([10, 20]);
        assert_eq!(
            gas.checked_scalar_product(10).unwrap(),
            GasUnit::<2>::from([100, 200]),
        );

        let gas = GasUnit::<2>::from([u64::MAX, 20]);
        assert!(gas.checked_scalar_product(10).is_none());

        let gas = GasUnit::<2>::from([10, u64::MAX]);
        assert!(gas.checked_scalar_product(10).is_none());

        let gas = GasUnit::<2>::from([u64::MAX, u64::MAX]);
        assert!(gas.checked_scalar_product(10).is_none());

        let gas = GasUnit::<2>::from([u64::MAX, u64::MAX]);
        assert_eq!(
            gas.checked_scalar_product(0).unwrap(),
            GasUnit::<2>::from([0, 0]),
        );
    }

    #[test]
    fn checked_combine_test() {
        let gas_1 = GasUnit::<2>::from([10, 20]);
        let gas_2 = GasUnit::<2>::from([10, 20]);

        assert_eq!(
            gas_1.checked_combine(&gas_2).unwrap(),
            GasUnit::<2>::from([20, 40]),
            "The gas unit should be combined correctly"
        );

        let gas_1 = GasUnit::<2>::from([u64::MAX, 20]);
        let gas_2 = GasUnit::<2>::from([10, 20]);

        assert!(gas_1.checked_combine(&gas_2).is_none());

        let gas_1 = GasUnit::<2>::from([20, 20]);
        let gas_2 = GasUnit::<2>::from([10, u64::MAX]);

        assert!(gas_1.checked_combine(&gas_2).is_none());

        let gas_1 = GasUnit::<2>::from([u64::MAX, u64::MAX]);
        let gas_2 = GasUnit::<2>::from([10, 20]);

        assert!(gas_1.checked_combine(&gas_2).is_none());

        let gas_1 = GasUnit::<2>::from([u64::MAX, u64::MAX]);
        let gas_2 = GasUnit::<2>::from([u64::MAX, u64::MAX]);

        assert!(gas_1.checked_combine(&gas_2).is_none());
    }

    #[test]
    fn checked_value_test() {
        let gas = GasUnit::<2>::from([10, 20]);
        let gas_price = GasPrice::<2>::from([3, 5]);

        let value = gas.checked_value(&gas_price).unwrap();
        assert_eq!(value, 130);

        let gas = GasUnit::<2>::from([u64::MAX, 20]);
        let gas_price = GasPrice::<2>::from([3, 5]);

        let value = gas.checked_value(&gas_price);
        assert!(value.is_none());

        let gas = GasUnit::<2>::from([u64::MAX, 1]);
        let gas_price = GasPrice::<2>::from([1, 1]);
        let value = gas.checked_value(&gas_price);
        assert!(value.is_none());

        let gas = GasUnit::<2>::from([0, 10]);
        let gas_price = GasPrice::<2>::from([u64::MAX, 20]);

        let value = gas.checked_value(&gas_price).unwrap();
        assert_eq!(value, 200);
    }

    #[test]
    fn charge_gas_should_fail_if_not_enough_funds() {
        let gas_price = GasPrice::<2>::from([1; 2]);

        {
            let mut gas_meter =
                BasicGasMeter::<S>::new_with_gas(GasUnit::<2>::ZEROED, gas_price.clone());
            assert!(
                gas_meter.charge_gas(&GasUnit::<2>::from([100; 2])).is_err(),
                "The gas meter should not be able to charge gas if there is not enough funds"
            );
        }

        {
            let gas = GasUnit::<2>::from([0, 0]);
            let mut gas_meter = BasicGasMeter::<S>::new_with_gas(gas, gas_price.clone());

            assert!(
                gas_meter.charge_gas(&GasUnit::<2>::from([100; 2])).is_err(),
                "The gas meter should not be able to charge gas if there is not enough gas reserved"
            );

            let gas = GasUnit::<2>::from([1000, 99]);
            let mut gas_meter = BasicGasMeter::<S>::new_with_gas(gas, gas_price.clone());

            assert!(
                gas_meter.charge_gas(&GasUnit::<2>::from([100; 2])).is_err(),
                "The gas meter should not be able to charge gas if there is not enough gas reserved"
            );
        }
    }

    #[test]
    fn refund_gas_should_fail_if_not_enough_funds_consumed() {
        let gas_price = GasPrice::<2>::from([1; 2]);

        {
            let mut gas_meter = BasicGasMeter::<S>::new_with_funds_and_gas(
                100,
                GasUnit::<2>::MAX,
                gas_price.clone(),
            );

            assert!(
            gas_meter.refund_gas(&GasUnit::<2>::from([100; 2])).is_err(),
            "The gas meter should not be able to refund gas if there is not enough gas consumed"
        );
        }

        {
            let mut gas_meter =
                BasicGasMeter::<S>::new_with_gas(GasUnit::<2>::from([100; 2]), gas_price.clone());

            assert!(
            gas_meter.refund_gas(&GasUnit::<2>::from([10; 2])).is_err(),
            "The gas meter should not be able to refund gas if there is not enough gas consumed"
            );
        }
    }

    #[test]
    fn try_charge_gas() {
        {
            const REMAINING_FUNDS: u64 = 100;
            let gas_price = GasPrice::<2>::from([1; 2]);

            let mut gas_meter = BasicGasMeter::<S>::new_with_funds_and_gas(
                REMAINING_FUNDS,
                GasUnit::<2>::MAX,
                gas_price.clone(),
            );
            assert!(
                gas_meter
                    .charge_gas(&GasUnit::<2>::from([REMAINING_FUNDS / 2; 2]))
                    .is_ok(),
                "It should be possible to charge gas"
            );
            assert_eq!(
                gas_meter.gas_info().gas_used,
                GasUnit::from([REMAINING_FUNDS / 2; 2]),
                "The gas used should be the same as the gas charged"
            );
            assert_eq!(gas_meter.gas_info().gas_price, gas_price);
            assert_eq!(
                gas_meter.gas_info().remaining_funds,
                0,
                "There should be no more gas left in the meter"
            );

            assert!(
            gas_meter.charge_gas(&GasUnit::<2>::from([1; 2])).is_err(),
            "There should be no more gas left in the meter, hence charging more gas should fail"
        );
        }

        {
            let remaining_gas = GasUnit::<2>::from([100; 2]);
            let gas_price = GasPrice::<2>::from([1; 2]);

            let mut gas_meter =
                BasicGasMeter::<S>::new_with_gas(remaining_gas.clone(), gas_price.clone());

            assert!(
                gas_meter.charge_gas(&remaining_gas.clone()).is_ok(),
                "It should be possible to charge gas"
            );
            assert_eq!(
                gas_meter.gas_info().gas_used,
                remaining_gas,
                "The gas used should be the same as the gas charged"
            );
            assert_eq!(gas_meter.gas_info().gas_price, gas_price);

            assert_eq!(
                gas_meter.gas_info().remaining_funds,
                0,
                "There should be no more gas left in the meter"
            );

            assert!(
                gas_meter.charge_gas(&GasUnit::<2>::from([1; 2])).is_err(),
                "There should be no more gas left in the meter, hence charging more gas should fail"
            );
        }
    }

    #[test]
    fn try_refund_gas_with_funds() {
        const REMAINING_FUNDS: u64 = 100;
        let gas_price = GasPrice::from([1; 2]);

        let mut gas_meter = BasicGasMeter::<S>::new_with_funds_and_gas(
            REMAINING_FUNDS,
            GasUnit::<2>::MAX,
            gas_price,
        );
        assert!(
            gas_meter
                .charge_gas(&GasUnit::<2>::from([REMAINING_FUNDS / 2; 2]))
                .is_ok(),
            "There should be enough gas left in the meter to charge"
        );

        assert_eq!(
            gas_meter.gas_info().remaining_funds,
            0,
            "There should be no more gas left in the meter"
        );

        assert!(
            gas_meter
                .refund_gas(&GasUnit::from([REMAINING_FUNDS / 4; 2]))
                .is_ok(),
            "Enough gas should have been consumed to be refunded",
        );

        assert_eq!(
            &gas_meter.gas_info().gas_used,
            &GasUnit::from([REMAINING_FUNDS / 4; 2],),
            "The gas used amount should have decreased"
        );

        assert_eq!(
            gas_meter.gas_info().remaining_funds,
            REMAINING_FUNDS / 2,
            "Half of the gas should be refunded"
        );
    }

    #[test]
    fn try_refund_gas() {
        let remaining_gas = GasUnit::<2>::from([100; 2]);
        let gas_price = GasPrice::<2>::from([1; 2]);

        let mut gas_meter =
            BasicGasMeter::<S>::new_with_gas(remaining_gas.clone(), gas_price.clone());

        assert!(
            gas_meter.charge_gas(&GasUnit::<2>::from([100; 2])).is_ok(),
            "There should be enough gas left in the meter to charge"
        );

        assert_eq!(
            gas_meter.gas_info().remaining_funds,
            0,
            "There should be no more gas left in the meter"
        );

        assert!(
            gas_meter.refund_gas(&GasUnit::<2>::from([25; 2])).is_ok(),
            "Enough gas should have been consumed to be refunded",
        );

        assert_eq!(
            &gas_meter.gas_info().gas_used,
            &GasUnit::<2>::from([75; 2]),
            "The gas used amount should have decreased"
        );

        assert_eq!(
            gas_meter.gas_info().remaining_funds,
            50,
            "Half of the gas should be refunded"
        );
    }

    #[test]
    fn gas_meter_charge_gas_overflow_test() {
        let remaining_gas = GasUnit::<2>::from([u64::MAX, u64::MAX]);
        let gas_price = GasPrice::<2>::from([u64::MAX; 2]);

        let mut gas_meter =
            BasicGasMeter::<S>::new_with_gas(remaining_gas.clone(), gas_price.clone());

        let gas = GasUnit::<2>::from([2; 2]);
        let res = gas_meter.charge_gas(&gas.clone());

        assert_eq!(
            res,
            Err(GasMeteringError::Overflow(
                "Charge Gas: Unable to charge gas, because the calculation overflows".to_string()
            ))
        );

        let mut gas_meter = BasicGasMeter::<S>::new_with_funds_and_gas(
            u64::MAX,
            remaining_gas.clone(),
            gas_price.clone(),
        );

        let res = gas_meter.charge_gas(&gas);

        assert_eq!(
            res,
            Err(GasMeteringError::Overflow(
                "Charge Funds: Unable to charge gas, because the calculation overflows".to_string()
            ))
        );
    }

    #[test]
    fn gas_meter_refund_gas_overflow_test() {
        let mut gas_meter = BasicGasMeter::<S> {
            initial_gas: GasUnit::<2>::from([u64::MAX, u64::MAX]),
            remaining_gas: GasUnit::<2>::ZEROED,
            remaining_funds: Some(0),
            gas_price: GasPrice::<2>::from([u64::MAX; 2]),
        };

        let res = gas_meter.refund_gas(&GasUnit::<2>::from([2, 2]));
        assert_eq!(
            res,
            Err(GasMeteringError::Overflow(
                "Refund Gas: Unable to refund gas, because the calculation overflows".to_string()
            ))
        );
    }

    #[test]
    fn slot_gas_meter_test() {
        let mut slot_gas_meter = SlotGasMeter::<S>::new(GasUnit::<2>::from([100, 200]), None);
        let sequencer = MockAddress::new([10; 32]);

        let gas = GasUnit::<2>::from([10, 20]);
        slot_gas_meter.charge_gas(&gas, &sequencer).unwrap();

        assert_eq!(
            slot_gas_meter.remaining_slot_gas(&sequencer),
            &GasUnit::<2>::from([90, 180])
        );

        let preferred_sequencer = MockAddress::new([10; 32]);
        let mut slot_gas_meter =
            SlotGasMeter::<S>::new(GasUnit::<2>::from([100, 200]), Some(preferred_sequencer));

        let sequencer = MockAddress::new([33; 32]);

        let gas = GasUnit::<2>::from([10, 20]);
        slot_gas_meter
            .charge_gas(&gas, &preferred_sequencer)
            .unwrap();

        let expected_preferred_gas = &GasUnit::<2>::from([80, 160]);
        assert_eq!(
            slot_gas_meter.remaining_slot_gas(&preferred_sequencer),
            expected_preferred_gas
        );

        let gas = GasUnit::<2>::from([20, 30]);
        slot_gas_meter.charge_gas(&gas, &sequencer).unwrap();

        assert_eq!(
            slot_gas_meter.remaining_slot_gas(&sequencer),
            &GasUnit::<2>::from([70, 150])
        );

        assert_eq!(
            &slot_gas_meter.remaining_preferred_slot_gas,
            expected_preferred_gas
        );
    }
}
