//! Gas unit definitions and implementations.

use core::fmt::{self, Debug, Display};

use anyhow::Result;
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "native")]
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_modules_macros::config_value_private;
use thiserror::Error;

const GAS_DIMENSIONS: usize = config_value_private!(
    "GAS_DIMENSIONS",
    "Couldn't parse `GAS_DIMENSIONS` in TOML file; must be a constant integer (e.g. `GAS_DIMENSIONS = { const = 2 }`)"
);

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

/// Contain information about the gas usage of a gas.
pub struct GasInfo<GU: Gas> {
    /// The current gas used accumulated by the stake meter.
    pub gas_used: GU,
    /// The current gas price
    pub gas_price: GU::Price,
    /// The remaining amount of tokens locked in the meter
    pub remaining_funds: u64,
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

    /// Returns gas usage.
    fn gas_info(&self) -> GasInfo<GU>;
}

/// A struct that keeps track of the use gas.
#[derive(Clone)]
pub struct BasicGasMeter<GU: Gas> {
    remaining_funds: u64,
    gas_used: GU,
    gas_price: GU::Price,
}

impl<GU: Gas> BasicGasMeter<GU> {
    /// Creates a new `BasicGasMeter`
    pub fn new(remaining_funds: u64, gas_price: GU::Price) -> Self {
        Self {
            remaining_funds,
            gas_used: Gas::zero(),
            gas_price,
        }
    }
}

impl<GU: Gas> GasMeter<GU> for BasicGasMeter<GU> {
    fn charge_gas(&mut self, amount: &GU) -> Result<(), GasMeteringError<GU>> {
        let amount_value = amount.value(&self.gas_price);

        if amount_value > self.remaining_funds {
            return Err(GasMeteringError::OutOfGas {
                gas_to_charge: amount.clone(),
                gas_price: self.gas_price.clone(),
                remaining_funds: self.remaining_funds,
                total_gas_consumed: self.gas_info().gas_used,
            });
        }

        self.remaining_funds -= amount_value;
        self.gas_used.combine(amount);

        Ok(())
    }

    fn refund_gas(&mut self, gas: &GU) -> Result<(), GasMeteringError<GU>> {
        self.gas_used = self.gas_used.checked_sub(gas).ok_or_else(|| {
            GasMeteringError::ImpossibleToRefundGas {
                gas_to_refund: gas.clone(),
                gas_used: self.gas_used.clone(),
            }
        })?;
        self.remaining_funds = self
            .remaining_funds
            .saturating_add(gas.value(&self.gas_price));

        Ok(())
    }

    fn gas_info(&self) -> GasInfo<GU> {
        GasInfo {
            gas_used: self.gas_used.clone(),
            gas_price: self.gas_price.clone(),
            remaining_funds: self.remaining_funds,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charge_gas_should_fail_if_not_enough_funds() {
        let gas_price = GasPrice::<2>::from([1; 2]);

        let mut gas_meter = BasicGasMeter::new(0, gas_price.clone());

        assert!(
            gas_meter.charge_gas(&GasUnit::<2>::from([100; 2])).is_err(),
            "The gas meter should not be able to charge gas if there is not enough funds"
        );
    }

    #[test]
    fn refund_gas_should_fail_if_not_enough_funds_consumed() {
        let gas_price = GasPrice::<2>::from([1; 2]);

        let mut gas_meter = BasicGasMeter::new(100, gas_price.clone());

        assert!(
            gas_meter.refund_gas(&GasUnit::<2>::from([100; 2])).is_err(),
            "The gas meter should not be able to refund gas if there is not enough gas consumed"
        );
    }

    #[test]
    fn try_charge_gas() {
        const REMAINING_FUNDS: u64 = 100;
        let gas_price = GasPrice::<2>::from([1; 2]);

        let mut gas_meter = BasicGasMeter::new(REMAINING_FUNDS, gas_price.clone());
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

    #[test]
    fn try_refund_gas() {
        const REMAINING_FUNDS: u64 = 100;
        let gas_price = GasPrice::from([1; 2]);

        let mut gas_meter = BasicGasMeter::new(REMAINING_FUNDS, gas_price);
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
}
