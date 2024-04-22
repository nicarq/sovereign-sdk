//! Gas unit definitions and implementations.

use alloc::vec::Vec;
use core::fmt;

use anyhow::Result;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// A multi-dimensional gas unit represented as an array of `u64`.`
pub trait GasArray:
    fmt::Debug
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
{
    /// A zeroed instance of the unit.
    const ZEROED: Self;

    /// Creates a unit from a multi-dimensional unit with arbitrary dimension.
    fn from_slice(dimensions: &[u64]) -> Self;

    /// Returns a multi-dimensional representation of the unit.
    fn as_slice(&self) -> &[u64];

    /// Returns a mutable reference to the multi-dimensional representation of the unit.
    fn as_slice_mut(&mut self) -> &mut [u64];

    /// Creates a multi-dimensional representation of the unit.
    fn to_vec(&self) -> Vec<u64>;

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

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
/// A multi-dimensional gas unit.
pub struct GasUnit<const N: usize>([u64; N]);
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
/// A gas price for multi-dimensional gas.
pub struct GasPrice<const N: usize>([u64; N]);

macro_rules! impl_gas_dimensions {
    ($t: ty, $n: expr) => {
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

        impl GasArray for $t {
            const ZEROED: Self = Self([0; $n]);

            fn from_slice(dimensions: &[u64]) -> Self {
                // as demonstrated on the link below, the compiler can easily optimize the conversion as if
                // it is a transparent type.
                //
                // https://rust.godbolt.org/z/rPhaxnPEY
                let mut unit = Self::ZEROED;
                unit.0
                    .iter_mut()
                    .zip(dimensions.iter().copied())
                    .for_each(|(a, b)| *a = b);
                unit
            }

            fn as_slice(&self) -> &[u64] {
                &self.0[..]
            }

            fn as_slice_mut(&mut self) -> &mut [u64] {
                &mut self.0[..]
            }

            fn to_vec(&self) -> Vec<u64> {
                self.0.to_vec()
            }

            fn checked_sub(&self, rhs: &Self) -> Option<Self> {
                let res: Option<Vec<u64>> = self
                    .0
                    .as_slice()
                    .iter()
                    .zip(rhs.0.as_slice())
                    .map(|(l, r)| l.checked_sub(*r))
                    .collect();

                res.map(|v| Self::from_slice(&v))
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
                    .zip(price.as_slice().iter().copied())
                    .map(|(a, b)| a.saturating_mul(b))
                    .fold(0, |a, b| a.saturating_add(b))
            }
        }

        impl_gas_dimensions!(GasUnit<$n>, $n);
        impl_gas_dimensions!(GasPrice<$n>, $n);
    };
}

impl_gas_unit!(1);
impl_gas_unit!(2);
impl_gas_unit!(3);
impl_gas_unit!(4);
impl_gas_unit!(5);
impl_gas_unit!(6);
impl_gas_unit!(7);
impl_gas_unit!(8);
impl_gas_unit!(9);
impl_gas_unit!(10);
impl_gas_unit!(11);
impl_gas_unit!(12);
impl_gas_unit!(13);
impl_gas_unit!(14);
impl_gas_unit!(15);
impl_gas_unit!(16);
impl_gas_unit!(17);
impl_gas_unit!(18);
impl_gas_unit!(19);
impl_gas_unit!(20);
impl_gas_unit!(21);
impl_gas_unit!(22);
impl_gas_unit!(23);
impl_gas_unit!(24);
impl_gas_unit!(25);
impl_gas_unit!(26);
impl_gas_unit!(27);
impl_gas_unit!(28);
impl_gas_unit!(29);
impl_gas_unit!(30);
impl_gas_unit!(31);
impl_gas_unit!(32);

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
        GasPrice::from_slice(&[value])
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
        GasUnit::from_slice(&[value])
    }
}

/// A gas meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GasMeter<GU>
where
    GU: Gas,
{
    remaining_funds: u64,
    gas_price: GU::Price,
    gas_used: GU,
}

impl<GU> Default for GasMeter<GU>
where
    GU: Gas,
{
    fn default() -> Self {
        Self {
            remaining_funds: 0,
            gas_price: GU::Price::ZEROED,
            gas_used: GU::ZEROED,
        }
    }
}

impl<GU> GasMeter<GU>
where
    GU: Gas,
{
    /// Creates a new instance of the gas meter with the provided price.
    pub fn new(remaining_funds: u64, gas_price: GU::Price) -> Self {
        Self {
            remaining_funds,
            gas_price,
            gas_used: GU::ZEROED,
        }
    }

    /// Returns the remaining gas funds.
    pub const fn remaining_funds(&self) -> u64 {
        self.remaining_funds
    }

    /// Returns the total gas incurred.
    pub const fn gas_used(&self) -> &GU {
        &self.gas_used
    }

    /// Returns the gas price.
    pub const fn gas_price(&self) -> &GU::Price {
        &self.gas_price
    }

    /// Overrides the current gas funds available for transaction execution.
    pub fn set_gas_funds(&mut self, funds: u64) {
        self.remaining_funds = funds;
        self.gas_used = GU::ZEROED;
    }

    /// Overrides the current gas price for transaction execution.
    pub fn set_gas_price(&mut self, gas_price: GU::Price) {
        self.gas_price = gas_price;
    }

    /// Deducts the provided gas unit from the remaining funds, computing the scalar value of the
    /// funds from the price of the instance.
    pub fn charge_gas(&mut self, gas: &GU) -> Result<()> {
        // Check that there's enough gas to cover the cost before mutating the gas_used counter.
        // This ensures that in the corner case where...
        //  - User wants to do expensive operation
        //  - User does not have enough gas left
        // ... the check fails and the user does not lose any gas - which is what we want
        // since the operation won't be performed.
        //
        // This also ensures that the `gas_used` stays in sync with the `remaining_funds` counter.
        let funds_to_charge = gas.value(&self.gas_price);
        self.remaining_funds = self
            .remaining_funds
            .checked_sub(funds_to_charge)
            .ok_or_else(|| anyhow::anyhow!("Not enough gas"))?;

        self.gas_used.combine(gas);

        Ok(())
    }

    /// Returns a gas meter which does not charge for gas.
    pub fn unmetered() -> Self {
        Self {
            remaining_funds: u64::MAX,
            gas_price: GU::Price::ZEROED,
            gas_used: GU::ZEROED,
        }
    }
}
