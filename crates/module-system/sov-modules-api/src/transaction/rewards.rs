use std::cmp::min;
use std::fmt::{Display, Formatter};

use borsh::{BorshDeserialize, BorshSerialize};
use sov_modules_macros::config_value;

use super::data::PriorityFeeBips;
use crate::{Gas, GasArray, Spec};

/// The format of the resources consumed by the transaction. The base fee and the priority fee are expressed as gas token amounts.
/// The [`TransactionConsumption`] data structure can only be built from the [`crate::WorkingSet`] data structure.
///
/// ## Type safety
/// To build this data structure outside of `sov-modules-api`, one would need to call [`crate::WorkingSet::finalize`] or [`crate::WorkingSet::checkpoint`]
#[derive(PartialEq, Eq, Debug)]
pub struct TransactionConsumption<GU: Gas> {
    /// The amount of funds locked in the transaction that remains after transaction is executed and tip is processed.
    /// This amount includes the `base_fee` and the `priority_fee` gas token consumption
    pub(crate) remaining_funds: u64,
    /// The base fee reward of the transaction expressed as a gas token amount.
    pub(crate) base_fee: GU,
    /// The priority fee reward of the transaction expressed as a gas token amount.
    pub(crate) priority_fee: u64,
    /// The gas price of the transaction.
    pub(crate) gas_price: GU::Price,
}

impl<GU: Gas> TransactionConsumption<GU> {
    /// A zero consumption. Happens when the transaction is ignored (like in the case of a revert for the speculative execution mode).
    pub const ZERO: Self = Self {
        remaining_funds: 0,
        base_fee: GU::ZEROED,
        priority_fee: 0,
        gas_price: GU::Price::ZEROED,
    };

    /// The base fee reward of the transaction expressed as a gas token amount.
    pub const fn base_fee(&self) -> &GU {
        &self.base_fee
    }

    pub fn base_fee_value(&self) -> u64 {
        self.base_fee.value(&self.gas_price)
    }

    /// The priority fee reward of the transaction expressed as a gas token amount.
    pub const fn priority_fee(&self) -> u64 {
        self.priority_fee
    }

    /// If the total consumption overflows, we saturate, because we know that this amount will always be lower than the max fee.
    pub fn total_consumption(&self) -> u64 {
        self.base_fee
            .value(&self.gas_price)
            .saturating_add(self.priority_fee)
    }

    pub fn remaining_funds(&self) -> u64 {
        self.remaining_funds
    }
}

impl<GU: Gas> Display for TransactionConsumption<GU> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TransactionConsumption {{ remaining_funds: {}, base_fee: {}, priority_fee: {}, gas_price: {} }}",
            self.remaining_funds, self.base_fee, self.priority_fee, self.gas_price
        )
    }
}

/// The type used to represent the sequencer reward. This type should be obtained from the [`TransactionConsumption`] type.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct SequencerReward(pub u64);

impl SequencerReward {
    /// Returns a zero sequencer reward. This can be used to initialize an accumulator to build a sequencer reward.
    pub const ZERO: Self = Self(0);

    /// Adds another reward to this reward. Consumes the other reward.
    /// If the result overflows, we saturate.
    pub fn accumulate(&mut self, other: Self) {
        self.0 = self.0.saturating_add(other.0);
    }
}

impl<GU: Gas> From<TransactionConsumption<GU>> for SequencerReward {
    fn from(value: TransactionConsumption<GU>) -> Self {
        Self(value.priority_fee())
    }
}

impl From<SequencerReward> for u64 {
    fn from(val: SequencerReward) -> Self {
        val.0
    }
}

impl Display for SequencerReward {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SequencerReward({})", self.0)
    }
}

/// Computes the transaction consumption for a given transaction.
/// This function is only used by the [`crate::WorkingSet`] to build a [`TransactionConsumption`] at the end of a transaction execution.
pub(crate) fn transaction_consumption_helper<S: Spec>(
    base_fee: &S::Gas,
    gas_price: &<S::Gas as Gas>::Price,
    max_fee: u64,
    max_priority_fee_bips: PriorityFeeBips,
) -> TransactionConsumption<S::Gas> {
    let base_fee_value = base_fee.value(gas_price);

    // We compute the `max_priority_fee_bips` by applying the `priority_fee_per_gas` to the consumed gas.
    let max_priority_fee_bips = max_priority_fee_bips
        .apply(base_fee_value)
        // if the computation overflows, we return the max fee - we always have `priority_fee <= tx.max_priority_fee_bips() <= tx.max_fee()`
        .unwrap_or(max_fee);

    // The tip is the minimum of the remaining gas allocated to the transaction and the maximum priority fee per gas.
    // We transfer the tip to the tip recipient address.
    let tip = min(max_priority_fee_bips, max_fee - base_fee_value);

    // Since the tip is an amount of gas tokens consumed on top of the base fee from the gas meter, we need to take that into
    // account in the calculation.
    let remaining_funds_including_tip = max_fee.saturating_sub(base_fee_value).saturating_sub(tip);

    TransactionConsumption {
        remaining_funds: remaining_funds_including_tip,
        base_fee: base_fee.clone(),
        priority_fee: tip,
        gas_price: gas_price.clone(),
    }
}

pub fn forced_sequencer_registration_cost<S: Spec>() -> S::Gas {
    const GAS_FORCED_SEQUENCER_REGISTRATION_COST: [u64; 2] =
        config_value!("GAS_FORCED_SEQUENCER_REGISTRATION_COST");

    S::Gas::from_slice(&GAS_FORCED_SEQUENCER_REGISTRATION_COST)
}
