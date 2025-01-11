use std::collections::BTreeMap;
use std::rc::Rc;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::transaction::Transaction;
use crate::{BasicGasMeter, DispatchCall, Gas, GasMeteringError, Spec};

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
    pub const fn from_percentage(value: u64) -> Self {
        Self(value * 100)
    }
}

#[derive(Debug, thiserror::Error)]
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
    /// Applies the priority fee to a given quantity
    /// We make sure to cast the intermediate result to u128 to avoid overflowing.
    pub fn apply(&self, quantity: u64) -> Result<u64, PriorityFeeApplyOverflowError> {
        // We need to cast to u128 to avoid overflowing.
        let quantity_u128 = quantity as u128;
        let fee_u128 = self.0 as u128;
        let result = (quantity_u128 * fee_u128) / (10_000);
        result.try_into().map_err(|_| PriorityFeeApplyOverflowError)
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
    pub max_fee: u64,
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
    pub max_fee: u64,
    /// The estimated gas usage of the transaction
    pub gas_limit: Option<S::Gas>,
}

impl<S: Spec> AuthenticatedTransactionData<S> {
    /// Creates a new [`BasicGasMeter`] from the transaction data.
    pub fn gas_meter(
        &self,
        gas_price: &<S::Gas as Gas>::Price,
        slot_gas_limit: S::Gas,
    ) -> Result<BasicGasMeter<S>, GasMeteringError<S::Gas>> {
        let gas_meter = match &self.gas_limit {
            Some(gas_limit) =>
            {
                #[allow(clippy::comparison_chain)]
                if *gas_limit < slot_gas_limit {
                    BasicGasMeter::new(self.max_fee, gas_limit.clone(), gas_price.clone())
                } else if *gas_limit > slot_gas_limit {
                    BasicGasMeter::new(self.max_fee, slot_gas_limit, gas_price.clone())
                } else {
                    return Err(GasMeteringError::SlotOutOfGas);
                }
            }
            None => BasicGasMeter::new(self.max_fee, slot_gas_limit, gas_price.clone()),
        };

        Ok(gas_meter)
    }
}
