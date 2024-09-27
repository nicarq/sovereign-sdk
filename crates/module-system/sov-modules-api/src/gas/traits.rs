//! Gas unit definitions and implementations.

use core::fmt::{self, Debug, Display};

use anyhow::Result;
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "native")]
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_modules_macros::config_value;
use thiserror::Error;

const GAS_DIMENSIONS: usize = config_value!("GAS_DIMENSIONS");

/// A multi-dimensional gas unit represented as an array of `u64`.`
#[cfg(feature = "native")]
pub trait GasArray:
    'static
    + fmt::Debug
    + Display
    + Clone
    + Send
    + Sync
    + PartialEq
    + Eq
    + PartialOrd
    + Ord
    + JsonSchema
    + core::hash::Hash
    + Serialize
    + DeserializeOwned
    + BorshSerialize
    + BorshDeserialize
    + From<[u64; GAS_DIMENSIONS]>
    + Into<[u64; GAS_DIMENSIONS]>
    + AsRef<[u64; GAS_DIMENSIONS]>
    + AsMut<[u64; GAS_DIMENSIONS]>
    + TryFrom<Vec<u64>, Error: Into<anyhow::Error> + Debug>
{
    /// A zeroed instance of the unit.
    const ZEROED: Self;

    /// In-place combination of gas units, resulting in an addition.
    fn combine(&mut self, rhs: &Self) -> &mut Self;

    /// Out-of-place substraction of gas units.
    ///
    /// # Output
    /// Returns [`None`] if the substraction in any gas dimension underflows.
    fn checked_sub(&self, rhs: &Self) -> Option<Self>;

    /// In-place division of gas units.
    fn scalar_division(&mut self, scalar: u64) -> &mut Self;

    /// In-place product of gas units, resulting in a multiplication.
    fn scalar_product(&mut self, scalar: u64) -> &mut Self;

    /// In-place addition of gas units with a scalar.
    fn scalar_add(&mut self, scalar: u64) -> &mut Self;

    /// In-place substraction of gas units with a scalar.
    fn scalar_sub(&mut self, scalar: u64) -> &mut Self;
}

/// A multi-dimensional gas unit represented as an array of `u64`.`
#[cfg(not(feature = "native"))]
pub trait GasArray:
    'static
    + fmt::Debug
    + Display
    + Clone
    + Send
    + Sync
    + PartialEq
    + Eq
    + PartialOrd
    + Ord
    + core::hash::Hash
    + Serialize
    + DeserializeOwned
    + BorshSerialize
    + BorshDeserialize
    + From<[u64; GAS_DIMENSIONS]>
    + Into<[u64; GAS_DIMENSIONS]>
    + AsRef<[u64; GAS_DIMENSIONS]>
    + AsMut<[u64; GAS_DIMENSIONS]>
    + TryFrom<Vec<u64>, Error: Into<anyhow::Error> + Debug>
{
    /// A zeroed instance of the unit.
    const ZEROED: Self;

    /// In-place combination of gas units, resulting in an addition.
    fn combine(&mut self, rhs: &Self) -> &mut Self;

    /// Out-of-place substraction of gas units.
    ///
    /// # Output
    /// Returns [`None`] if the substraction in any gas dimension underflows.
    fn checked_sub(&self, rhs: &Self) -> Option<Self>;

    /// In-place division of gas units.
    fn scalar_division(&mut self, scalar: u64) -> &mut Self;

    /// In-place product of gas units, resulting in a multiplication.
    fn scalar_product(&mut self, scalar: u64) -> &mut Self;

    /// In-place addition of gas units with a scalar.
    fn scalar_add(&mut self, scalar: u64) -> &mut Self;

    /// In-place substraction of gas units with a scalar.
    fn scalar_sub(&mut self, scalar: u64) -> &mut Self;
}

/// A unit of gas
pub trait Gas: GasArray {
    /// The price of the gas, expressed in tokens per unit.
    type Price: GasArray;

    /// Calculates the value of the given amount of gas at the given price.
    fn value(&self, price: &Self::Price) -> u64;

    /// Returns a gas unit which is zero in all dimensions.
    fn zero() -> Self {
        Self::ZEROED
    }
}

/// A multi-dimensional gas unit.
#[derive(
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    BorshSerialize,
    BorshDeserialize,
    derive_more::Display,
)]
#[display("GasUnit{:?}", self.0)]
pub struct GasUnit<const N: usize>([u64; N]);

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
    PartialOrd,
    Ord,
    BorshSerialize,
    BorshDeserialize,
    derive_more::Display,
)]
#[display("GasPrice{:?}", self.0)]
pub struct GasPrice<const N: usize>([u64; N]);

impl<const N: usize> Debug for GasPrice<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

macro_rules! impl_gas_dimensions {
    ($t: ty, $t_name: literal, $n: expr) => {
        #[cfg(feature = "native")]
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
                <[u64; $n] as serde::Serialize>::serialize(&self.0, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $t {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                <[u64; $n] as serde::Deserialize>::deserialize(deserializer).map(Self)
            }
        }

        impl From<[u64; $n]> for $t {
            fn from(array: [u64; $n]) -> Self {
                Self(array)
            }
        }

        impl From<$t> for [u64; $n] {
            fn from(gas: $t) -> [u64; $n] {
                gas.0
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

                Ok(Self(output))
            }
        }

        impl AsRef<[u64; $n]> for $t {
            fn as_ref(&self) -> &[u64; $n] {
                &self.0
            }
        }

        impl AsMut<[u64; $n]> for $t {
            fn as_mut(&mut self) -> &mut [u64; $n] {
                &mut self.0
            }
        }

        impl $t {
            /// Creates a new [`$t`] from an array of [`u64`].
            pub const fn from_primitive(array: [u64; $n]) -> Self {
                Self(array)
            }
        }

        impl GasArray for $t {
            const ZEROED: Self = Self([0; $n]);

            fn checked_sub(&self, rhs: &Self) -> Option<Self> {
                let mut output = [0; $n];

                for (i, (l, r)) in self.0.iter().zip(rhs.0.as_slice()).enumerate() {
                    if let Some(res) = l.checked_sub(*r) {
                        output[i] = res;
                    } else {
                        return None
                    }
                }

                Some(Self(output))
            }

            fn scalar_division(&mut self, scalar: u64) -> &mut Self {
                self.0
                    .iter_mut()
                    .for_each(|s| *s = s.checked_div(scalar).unwrap_or(0));
                self
            }

            fn scalar_product(&mut self, scalar: u64) -> &mut Self {
                self.0
                    .iter_mut()
                    .for_each(|s| *s = s.saturating_mul(scalar));
                self
            }

            fn scalar_add(&mut self, scalar: u64) -> &mut Self {
                self.0
                    .iter_mut()
                    .for_each(|s| *s = s.saturating_add(scalar));
                self
            }

            fn scalar_sub(&mut self, scalar: u64) -> &mut Self {
                self.0
                    .iter_mut()
                    .for_each(|s| *s = s.saturating_sub(scalar));
                self
            }

            fn combine(&mut self, rhs: &Self) -> &mut Self {
                self.0
                    .iter_mut()
                    .zip(rhs.0.iter())
                    .for_each(|(l, r)| *l = l.saturating_add(*r));
                self
            }
        }
    };
}

macro_rules! impl_gas_unit {
    ($n:expr) => {
        impl Gas for GasUnit<$n> {
            type Price = GasPrice<$n>;

            fn value(&self, price: &Self::Price) -> u64 {
                self.0
                    .iter()
                    .zip(price.as_ref().iter().copied())
                    .map(|(a, b)| a.saturating_mul(b))
                    .fold(0, |a, b| a.saturating_add(b))
            }
        }

        impl_gas_dimensions!(GasUnit<$n>, "GasUnit", $n);
        impl_gas_dimensions!(GasPrice<$n>, "GasPrice", $n);
    };
}

impl_gas_unit!(GAS_DIMENSIONS);

impl<'a> From<&'a GasPrice<1>> for u64 {
    fn from(value: &'a GasPrice<1>) -> Self {
        value.0[0]
    }
}

impl From<GasPrice<1>> for u64 {
    fn from(value: GasPrice<1>) -> Self {
        value.0[0]
    }
}

impl From<u64> for GasPrice<1> {
    fn from(value: u64) -> Self {
        GasPrice([value])
    }
}

impl<'a> From<&'a GasUnit<1>> for u64 {
    fn from(value: &'a GasUnit<1>) -> Self {
        value.0[0]
    }
}

impl From<GasUnit<1>> for u64 {
    fn from(value: GasUnit<1>) -> Self {
        value.0[0]
    }
}

impl From<u64> for GasUnit<1> {
    fn from(value: u64) -> Self {
        GasUnit([value])
    }
}

/// Error type that can be raised by the `GasMeter` trait.
/// Errors can be raised either when the meter runs out of gas or when the refund operation fails.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GasMeteringError<GU: Gas> {
    /// The gas meter has ran out of gas.
    #[error("The gas to charge is greater than the funds available in the meter. Gas to charge {gas_to_charge}, gas price {gas_price}, remaining funds {remaining_funds}, total gas consumed {total_gas_consumed}")]
    OutOfGas {
        /// The amount of gas to charge.
        gas_to_charge: GU,
        /// The current gas price.
        gas_price: GU::Price,
        /// The remaining funds in the meter.
        remaining_funds: u64,
        /// The total amount of gas consumed.
        total_gas_consumed: GU,
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

/// A type-safe trait that should track the gas consumed by a finite ressource over time.
pub trait GasMeter<GU: Gas> {
    /// Charges some gas in the gas meter.
    ///
    /// # Error
    /// May raises an error if the gas to charge is greater than the funds available
    fn charge_gas(&mut self, amount: &GU) -> Result<(), GasMeteringError<GU>>;

    /// Refunds some gas to the gas meter.
    ///
    /// ## Note
    /// This method may fail if the gas to refund is greater than the funds charged to the gas meter.
    /// In that case, the gas meter won't be updated and the refund will fail.
    fn refund_gas(&mut self, gas: &GU) -> Result<(), GasMeteringError<GU>>;

    /// Returns the current gas used accumulated by the stake meter.
    fn gas_used(&self) -> &GU;

    /// Returns the current gas price.
    fn gas_price(&self) -> &GU::Price;

    /// Returns the gas used as a token amount.
    fn gas_used_value(&self) -> u64 {
        self.gas_used().value(self.gas_price())
    }

    /// The remaining amount of tokens locked in the meter
    fn remaining_funds(&self) -> u64;
}

/// An unlimited gas meter. Only tracks the amount of gas consumed.
/// The [`UnlimitedGasMeter::charge_gas`] method will always succeed.
/// Only use this if you are certain that the gas meter will never run out of funds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnlimitedGasMeter<GU: Gas> {
    gas_used: GU,
    gas_price: GU::Price,
}

impl<GU: Gas> Default for UnlimitedGasMeter<GU> {
    fn default() -> Self {
        Self::new()
    }
}

impl<GU: Gas> UnlimitedGasMeter<GU> {
    /// Creates a new unlimited gas meter with the provided gas price.
    pub const fn new_with_price(gas_price: GU::Price) -> Self {
        Self {
            gas_used: GU::ZEROED,
            gas_price,
        }
    }
    /// Creates a new unlimited gas meter with a zeroed price.
    pub const fn new() -> Self {
        Self {
            gas_used: GU::ZEROED,
            gas_price: GU::Price::ZEROED,
        }
    }
}

impl<GU: Gas> GasMeter<GU> for UnlimitedGasMeter<GU> {
    fn charge_gas(&mut self, gas: &GU) -> Result<(), GasMeteringError<GU>> {
        self.gas_used.combine(gas);
        Ok(())
    }

    fn refund_gas(&mut self, gas: &GU) -> Result<(), GasMeteringError<GU>> {
        self.gas_used = self.gas_used.checked_sub(gas).ok_or_else(|| {
            GasMeteringError::ImpossibleToRefundGas {
                gas_to_refund: gas.clone(),
                gas_used: self.gas_used.clone(),
            }
        })?;

        Ok(())
    }

    fn gas_used(&self) -> &GU {
        &self.gas_used
    }

    /// Returns the gas price.
    fn gas_price(&self) -> &GU::Price {
        &self.gas_price
    }

    fn remaining_funds(&self) -> u64 {
        u64::MAX
    }
}
