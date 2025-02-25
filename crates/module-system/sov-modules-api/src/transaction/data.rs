use std::collections::BTreeMap;
use std::rc::Rc;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::transaction::Transaction;
use crate::{Amount, BasicGasMeter, DispatchCall, Gas, GasArray, Spec};

/// A type wrapper around a u64 which represents the priority fee.
/// Since the priority fee is expressed as a basis point, we should use this wrapper for
/// improved type safety.
///
/// # Note
/// The priority fee is expressed as a basis point. Ie, `1%` is represented as `10_000`.
#[derive(
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
)]
#[cfg_attr(
    feature = "native",
    derive(sov_rollup_interface::sov_universal_wallet::UniversalWallet)
)]
pub struct PriorityFeeBips(pub u64);

impl PriorityFeeBips {
    /// Creates a priority fee of zero. With a zero priority fee, the sequencer will not receive any reward for batch execution.
    pub const ZERO: Self = Self(0);

    /// Constant function to create a priority fee from a percentage.
    /// The priority fee is expressed as a basis point, ie `PriorityFeeBips(100)` is equivalent to a 1% fee -
    /// hence calling this `from_percentage(1)` will return `PriorityFeeBips(100)`.
    #[must_use]
    pub const fn from_percentage(value: u64) -> Self {
        Self(value * 100)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("Applying the priority fee to this quantity causes an overflow")]
pub struct PriorityFeeApplyOverflowError;

impl From<u64> for PriorityFeeBips {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<PriorityFeeBips> for u64 {
    fn from(value: PriorityFeeBips) -> Self {
        value.0
    }
}

impl PriorityFeeBips {
    /// Applies the priority fee to a given quantity if possible
    /// # Errors
    /// Returns an error in case of overflow.
    pub fn apply(&self, quantity: u128) -> Result<u128, PriorityFeeApplyOverflowError> {
        self.priority_fee_limbs(quantity)
    }

    fn priority_fee_limbs(&self, quantity: u128) -> Result<u128, PriorityFeeApplyOverflowError> {
        let hi = quantity >> 64;
        let lo = quantity & u64::MAX as u128;
        // Apply the fee to the high limb
        let hi_mul: u128 = hi * self.0 as u128;
        let mut hi_res = hi_mul / 10_000;
        let hi_rem = hi_mul % 10_000;

        // If the result overflows a u64,
        if hi_res > u64::MAX as u128 {
            return Err(PriorityFeeApplyOverflowError);
        }
        hi_res <<= 64;
        let res_lo = (lo * self.0 as u128) / 10_000;
        hi_res
            .checked_add(res_lo)
            .ok_or(PriorityFeeApplyOverflowError)?
            .checked_add((hi_rem << 64) / 10000)
            .ok_or(PriorityFeeApplyOverflowError)
    }
}

/// Contains details related to fees and gas handling.
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
)]
#[cfg_attr(
    feature = "native",
    derive(sov_rollup_interface::sov_universal_wallet::UniversalWallet)
)]
#[serde(bound = "S: Spec")]
pub struct TxDetails<S: Spec> {
    /// The maximum priority fee that can be paid for this transaction expressed as a basis point percentage of the gas consumed by the transaction.
    /// Ie if the transaction has consumed `100` gas tokens, and the priority fee is set to `100_000` (10%), the
    /// gas tip will be `10` tokens.
    pub max_priority_fee_bips: PriorityFeeBips,
    /// The maximum fee that can be paid for this transaction expressed as a the gas token amount
    pub max_fee: Amount,
    /// The gas limit of the transaction.
    /// This is an optional field that can be used to provide a limit of the gas usage of the transaction
    /// across the different gas dimensions. If provided, this quantity will be used along
    /// with the current gas price (`gas_limit *_scalar gas_price`) to compute the transaction fee and compare it to the `max_fee`.
    /// If the scalar product of the gas limit and the gas price is greater than the `max_fee`, the transaction will be rejected.
    /// Then up to `gas_limit *_scalar gas_price` gas tokens can be spent on gas execution in the transaction execution - if the
    /// transaction spends more than that amount, it will run out of gas and be reverted.
    pub gas_limit: Option<S::Gas>,
    /// The ID of the target chain.
    pub chain_id: u64,
}

impl<S: Spec> From<TxDetails<S>> for AuthenticatedTransactionData<S> {
    fn from(details: TxDetails<S>) -> Self {
        Self {
            chain_id: details.chain_id,
            max_priority_fee_bips: details.max_priority_fee_bips,
            max_fee: details.max_fee,
            gas_limit: details.gas_limit,
        }
    }
}

impl<T: DispatchCall, S: Spec> From<Transaction<T, S>> for AuthenticatedTransactionData<S> {
    fn from(tx: Transaction<T, S>) -> Self {
        tx.details.into()
    }
}

/// Holds the original credentials to authenticate the transaction.
/// For example, this could be a public key of the sender of the transaction.
#[derive(Clone, Debug, Default)]
pub struct Credentials {
    credentials: Rc<BTreeMap<core::any::TypeId, Rc<dyn core::any::Any>>>,
}

impl Credentials {
    /// Creates a new [`Credentials`] from the provided credential.
    pub fn new<T>(credential: T) -> Self
    where
        T: core::any::Any,
    {
        let mut map: BTreeMap<std::any::TypeId, Rc<dyn core::any::Any>> = BTreeMap::new();
        map.insert(core::any::TypeId::of::<T>(), Rc::new(credential));
        Self {
            credentials: Rc::new(map),
        }
    }

    /// Returns the relevant credential.
    #[must_use]
    pub fn get<T>(&self) -> Option<&T>
    where
        T: core::any::Any,
    {
        self.credentials
            .get(&core::any::TypeId::of::<T>())
            .and_then(|v| v.downcast_ref())
    }
}

/// Transaction data that has been authenticated.
/// This is the output of the `TransactionAuthenticator`.
pub struct AuthenticatedTransactionData<S: Spec> {
    /// The chain ID.
    pub chain_id: u64,
    /// The maximum priority fee that can be paid for this transaction expressed in bips.
    /// This priority fee is computed as a percentage of the total gas consumed by the transaction
    pub max_priority_fee_bips: PriorityFeeBips,
    /// The maximum fee that can be paid for this transaction expressed as a the gas token amount
    pub max_fee: Amount,
    /// The estimated gas usage of the transaction
    pub gas_limit: Option<S::Gas>,
}

impl<S: Spec> AuthenticatedTransactionData<S> {
    /// Creates a new [`BasicGasMeter`] from the transaction data.
    pub fn gas_meter(
        &self,
        gas_price: &<S::Gas as Gas>::Price,
        slot_gas_limit: &S::Gas,
    ) -> BasicGasMeter<S> {
        match &self.gas_limit {
            Some(gas_limit) => {
                // `GasArray::calculate_min` creates a new gas instance by selecting the minimum value along each dimension of the gas array.
                let new_gas_limit = <S::Gas as GasArray>::calculate_min(gas_limit, slot_gas_limit);
                BasicGasMeter::new_with_funds_and_gas(
                    self.max_fee,
                    new_gas_limit,
                    gas_price.clone(),
                )
            }
            None => BasicGasMeter::new_with_funds_and_gas(
                self.max_fee,
                slot_gas_limit.clone(),
                gas_price.clone(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_priority_fee_apply_basic() {
        let fee = PriorityFeeBips::from_percentage(100);
        let quantity = 1;
        let result = fee.apply(quantity);
        assert_eq!(result, Ok(1));
    }

    #[test]
    fn test_priority_fee_apply_basic_with_limbs() {
        let fee = PriorityFeeBips::from_percentage(43);
        let quantity = 100;
        let result = fee.apply(quantity);
        assert_eq!(result, Ok(43));
    }

    #[test]
    fn test_priority_fee_apply_would_overflow_without_limbs_basic() {
        let fee = PriorityFeeBips::from_percentage(100);
        let quantity = u128::MAX;
        let result = fee.apply(quantity);
        assert_eq!(result, Ok(u128::MAX));
    }

    #[test]
    fn test_priority_fee_apply_would_overflow_without_limbs_small_fee() {
        let fee = PriorityFeeBips::from_percentage(50);
        let quantity = u128::MAX;
        let result = fee.apply(quantity);
        assert_eq!(result, Ok(u128::MAX / 2));
    }

    #[test]
    fn test_priority_fee_apply_would_overflow_without_limbs_big_fee() {
        let fee = PriorityFeeBips::from_percentage(150);
        let quantity = u128::MAX / 2;
        let result = fee.apply(quantity);
        assert_eq!(result, Ok(255211775190703847597530955573826158590)); // Result calculated manually in Python
    }

    #[test]
    fn test_priority_fee_apply_overflows() {
        let fee = PriorityFeeBips::from_percentage(101);
        let quantity = u128::MAX;
        let result = fee.apply(quantity);
        assert_eq!(result, Err(PriorityFeeApplyOverflowError));
    }
}
