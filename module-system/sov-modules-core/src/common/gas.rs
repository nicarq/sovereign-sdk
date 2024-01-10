//! Gas unit definitions and implementations.

use alloc::vec::Vec;
use core::fmt;

use anyhow::Result;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// A gas unit that provides scalar conversion from complex, multi-dimensional types.
pub trait GasUnit:
    fmt::Debug
    + Clone
    + Send
    + Sync
    + PartialEq
    + Eq
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

    /// Converts the unit into a scalar value, given a price.
    fn value(&self, price: &Self) -> u64;

    /// In-place combination of gas units, resulting in an addition.
    fn combine(&mut self, rhs: &Self) -> &mut Self;

    /// In-place division of gas units.
    fn scalar_division(&mut self, scalar: u64) -> &mut Self;

    /// In-place product of gas units, resulting in a multiplication.
    fn scalar_product(&mut self, scalar: u64) -> &mut Self;

    /// Computes the elastic price, provided the arguments.
    ///
    /// The calculation of the base price for the current block execution determines the slope of
    /// the parents' gas target and gas consumption. This value is then multiplied by the parents'
    /// price and divided by the maximum elasticity factor.
    fn elastic_price(
        maximum_elasticity: i64,
        target: &Self,
        used: &Self,
        base_price: &Self,
        minimum_price: &Self,
    ) -> Self {
        let mut price = Self::ZEROED;

        price
            .as_slice_mut()
            .iter_mut()
            .zip(target.as_slice())
            .zip(used.as_slice())
            .zip(base_price.as_slice())
            .zip(minimum_price.as_slice())
            .for_each(|((((price, target), used), base_price), minimum_price)| {
                let target = *target as i64;
                let used = *used as i64;
                let base_price = *base_price as i64;

                // avoid undeterministic behavior of floating pointers
                let elasticity = target / maximum_elasticity;
                let factor = elasticity.saturating_add(used).saturating_sub(target);
                let value = base_price.saturating_mul(factor);
                let value = (value / elasticity) as u64;

                *price = (*minimum_price).max(value);
            });

        price
    }
}

/// A multi-dimensional gas unit.
pub type TupleGasUnit<const N: usize> = [u64; N];

macro_rules! impl_gas_unit {
    ($n:expr) => {
        impl GasUnit for TupleGasUnit<$n> {
            const ZEROED: Self = [0; $n];

            fn from_slice(dimensions: &[u64]) -> Self {
                // as demonstrated on the link below, the compiler can easily optimize the conversion as if
                // it is a transparent type.
                //
                // https://rust.godbolt.org/z/rPhaxnPEY
                let mut unit = Self::ZEROED;
                unit.iter_mut()
                    .zip(dimensions.iter().copied())
                    .for_each(|(a, b)| *a = b);
                unit
            }

            fn as_slice(&self) -> &[u64] {
                &self[..]
            }

            fn as_slice_mut(&mut self) -> &mut [u64] {
                &mut self[..]
            }

            fn to_vec(&self) -> Vec<u64> {
                <[u64]>::to_vec(self)
            }

            fn value(&self, price: &Self) -> u64 {
                self.iter()
                    .zip(price.iter().copied())
                    .map(|(a, b)| a.saturating_mul(b))
                    .fold(0, |a, b| a.saturating_add(b))
            }

            fn combine(&mut self, rhs: &Self) -> &mut Self {
                self.iter_mut()
                    .zip(rhs.iter())
                    .for_each(|(l, r)| *l = l.saturating_add(*r));
                self
            }

            fn scalar_division(&mut self, scalar: u64) -> &mut Self {
                self.iter_mut().for_each(|s| *s /= scalar);
                self
            }

            fn scalar_product(&mut self, scalar: u64) -> &mut Self {
                self.iter_mut().for_each(|s| *s = s.saturating_mul(scalar));
                self
            }
        }
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

/// A gas meter.
pub struct GasMeter<GU>
where
    GU: GasUnit,
{
    remaining_funds: u64,
    gas_price: GU,
    gas_used: GU,
}

impl<GU> Default for GasMeter<GU>
where
    GU: GasUnit,
{
    fn default() -> Self {
        Self {
            remaining_funds: 0,
            gas_price: GU::ZEROED,
            gas_used: GU::ZEROED,
        }
    }
}

impl<GU> GasMeter<GU>
where
    GU: GasUnit,
{
    /// Creates a new instance of the gas meter with the provided price.
    pub fn new(remaining_funds: u64, gas_price: GU) -> Self {
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
    pub const fn gas_price(&self) -> &GU {
        &self.gas_price
    }

    /// Overrides the current gas funds available for transaction execution.
    pub fn set_gas_funds(&mut self, funds: u64) {
        self.remaining_funds = funds;
        self.gas_used = GU::ZEROED;
    }

    /// Overrides the current gas price for transaction execution.
    pub fn set_gas_price(&mut self, gas_price: GU) {
        self.gas_price = gas_price;
    }

    /// Deducts the provided gas unit from the remaining funds, computing the scalar value of the
    /// funds from the price of the instance.
    pub fn charge_gas(&mut self, gas: &GU) -> Result<()> {
        self.gas_used.combine(gas);

        let gas = gas.value(&self.gas_price);
        self.remaining_funds = self
            .remaining_funds
            .checked_sub(gas)
            .ok_or_else(|| anyhow::anyhow!("Not enough gas"))?;

        Ok(())
    }
}

#[test]
fn gas_elastic_price_wont_overflow() {
    let elasticity = 1;
    let target = [2, 2];
    let used = [u64::MAX, u64::MAX];
    let base = [3, 3];
    let minimum = [1, 1];
    let price = GasUnit::elastic_price(elasticity, &target, &used, &base, &minimum);

    assert_eq!(price, used);
}

#[test]
fn gas_elastic_minimum_price_is_respected() {
    let elasticity = 1;
    let target = [13, 5];
    let used = [3, 2];
    let base = [7, 5];
    let minimum = [2, 2];
    let price = GasUnit::elastic_price(elasticity, &target, &used, &base, &minimum);

    assert_eq!(price, minimum);
}

#[test]
fn gas_elastic_price_will_decrease() {
    let elasticity = 1;
    let target = [17, 11];
    let used = [16, 11];
    let base = [10, 10];
    let minimum = [1, 1];
    let price = GasUnit::elastic_price(elasticity, &target, &used, &base, &minimum);

    assert_eq!(price, [9, 10]);
}

#[test]
fn gas_elastic_price_will_increase() {
    let elasticity = 1;
    let target = [17, 11];
    let used = [17, 13];
    let base = [10, 10];
    let minimum = [1, 1];
    let price = GasUnit::elastic_price(elasticity, &target, &used, &base, &minimum);

    assert_eq!(price, [10, 11]);
}

#[test]
fn gas_elasticity_increases_threshold() {
    let elasticity = 5;
    let target = [10, 10];
    let used = [100, 100];
    let base = [10, 10];
    let minimum = [1, 1];
    let price = GasUnit::elastic_price(elasticity, &target, &used, &base, &minimum);

    assert_eq!(price, [460, 460]);
}
