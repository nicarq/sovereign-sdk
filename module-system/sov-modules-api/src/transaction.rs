use std::collections::BTreeMap;
use std::sync::Arc;

use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_macros::cycle_tracker;
use serde::{Deserialize, Serialize};
use sov_modules_macros::config_value;
#[cfg(feature = "native")]
pub use sov_rollup_interface::crypto::PrivateKey;
use sov_rollup_interface::crypto::{PublicKey, Signature as _};
use sov_rollup_interface::zk::CryptoSpec;

use crate::{CredentialId, Gas, GasArray, GasMeter, Spec};

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
pub struct PriorityFeeBips(pub u64);

impl PriorityFeeBips {
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

/// A Transaction object that is compatible with the module-system/sov-default-stf.
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
pub struct Transaction<S: Spec> {
    pub signature: <S::CryptoSpec as CryptoSpec>::Signature,
    pub pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
    pub runtime_msg: Vec<u8>,
    pub chain_id: u64,
    /// The maximum priority fee that can be paid for this transaction expressed as a basis point percentage of the gas consumed by the transaction.
    /// Ie if the transaction has consumed `100` gas tokens, and the priority fee is set to `100_000` (10%), the
    /// gas tip will be `10` tokens.
    pub max_priority_fee_bips: PriorityFeeBips,
    /// The maximum fee that can be paid for this transaction expressed as a the gas token amount
    pub max_fee: u64,
    /// The gas limit of the transaction.
    /// This is an optional field that can be used to provide a limit of the gas usage of the transaction
    /// accross the different gas dimensions. If provided, this quantity will be used along
    /// with the current gas price (`gas_limit *_scalar gas_price`) to compute the transaction fee and compare it to the `max_fee`.
    /// If the scalar product of the gas limit and the gas price is greater than the `max_fee`, the transaction will be rejected.
    /// Then up to `gas_limit *_scalar gas_price` gas tokens can be spent on gas execution in the transaction execution - if the
    /// transaction spends more than that amount, it will run out of gas and be reverted.
    pub gas_limit: Option<S::Gas>,
    pub nonce: u64,
}

impl<S: Spec> Transaction<S> {
    pub fn signature(&self) -> &<S::CryptoSpec as CryptoSpec>::Signature {
        &self.signature
    }

    pub fn pub_key(&self) -> &<S::CryptoSpec as CryptoSpec>::PublicKey {
        &self.pub_key
    }

    pub fn runtime_msg(&self) -> &[u8] {
        &self.runtime_msg
    }

    /// Check whether the transaction has been signed correctly.
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    pub fn verify(&self) -> anyhow::Result<()> {
        let serialized_tx = self.to_unsigned_transaction().try_to_vec()?;
        self.signature().verify(&self.pub_key, &serialized_tx)?;

        Ok(())
    }

    /// New transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
        message: Vec<u8>,
        signature: <S::CryptoSpec as CryptoSpec>::Signature,
        chain_id: u64,
        max_priority_fee_bips: PriorityFeeBips,
        max_fee: u64,
        gas_limit: Option<S::Gas>,
        nonce: u64,
    ) -> Self {
        Self {
            signature,
            runtime_msg: message,
            pub_key,
            chain_id,
            max_priority_fee_bips,
            max_fee,
            gas_limit,
            nonce,
        }
    }

    fn to_unsigned_transaction(&self) -> UnsignedTransaction<S> {
        UnsignedTransaction::new(
            self.runtime_msg.clone(),
            self.chain_id,
            self.max_priority_fee_bips,
            self.max_fee,
            self.nonce,
            self.gas_limit.clone(),
        )
    }

    /// The gas cost to pay to perform pre-execution checks for a given transaction.
    /// Contains a fixed amount which corresponds to the cost of signature verification
    /// and a variable amount which corresponds to the cost of transaction deserialization/message decoding.
    ///
    /// TODO(@theochap): This method will be removed in the next PRs in favor of granular checks happening directly in the
    /// `Stf`
    pub fn gas_fixed_cost(&self) -> S::Gas {
        const GAS_TX_FIXED_COST: [u64; 2] = config_value!("GAS_TX_FIXED_COST");

        const GAS_TX_COST_PER_BYTE: [u64; 2] = config_value!("GAS_TX_COST_PER_BYTE");

        let gas_tx_fixed_cost = S::Gas::from_slice(&GAS_TX_FIXED_COST);
        let mut gas_tx_cost = S::Gas::from_slice(&GAS_TX_COST_PER_BYTE);

        gas_tx_cost.scalar_product(self.runtime_msg.len() as u64);
        gas_tx_cost.combine(&gas_tx_fixed_cost);

        gas_tx_cost
    }
}

#[cfg(feature = "native")]
impl<S: Spec> Transaction<S> {
    /// New signed transaction.
    pub fn new_signed_tx(
        priv_key: &<S::CryptoSpec as CryptoSpec>::PrivateKey,
        unsigned_tx: UnsignedTransaction<S>,
    ) -> Self {
        let mut utx_bytes: Vec<u8> = Vec::new();
        BorshSerialize::serialize(&unsigned_tx, &mut utx_bytes).unwrap();

        let pub_key = priv_key.pub_key();
        let signature = priv_key.sign(&utx_bytes);

        unsigned_tx.to_signed_tx(pub_key, signature)
    }
}

/// An unsent transaction with the required data to be submitted to the DA layer
#[derive(Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct UnsignedTransaction<S: Spec> {
    /// The runtime message
    pub runtime_msg: Vec<u8>,
    /// The ID of the target chain
    pub chain_id: u64,
    /// The maximum priority fee that can be paid for this transaction expressed in bips.
    /// This priority fee is computed as a percentage of the total gas consumed by the transaction
    pub max_priority_fee_bips: PriorityFeeBips,
    /// The maximum fee that can be paid for this transaction expressed as a the gas token amount
    pub max_fee: u64,
    /// The nonce
    pub nonce: u64,
    /// The estimated gas usage of the transaction
    /// This is an optional field that can be used to provide an estimate of the gas usage of the transaction
    /// across the different gas dimensions. If provided, this quantity will be used along
    /// with the current multi-dimensional gas price to compute the estimated transaction fee and compare it to the `max_fee`
    pub gas_limit: Option<S::Gas>,
}

impl<S: Spec> UnsignedTransaction<S> {
    /// Creates a new [`UnsignedTransaction`] with the given arguments.
    pub const fn new(
        runtime_msg: Vec<u8>,
        chain_id: u64,
        max_priority_fee_bips: PriorityFeeBips,
        max_fee: u64,
        nonce: u64,
        gas_limit: Option<S::Gas>,
    ) -> Self {
        Self {
            runtime_msg,
            chain_id,
            max_priority_fee_bips,
            max_fee,
            nonce,
            gas_limit,
        }
    }

    /// Creates a new [`Transaction`] from this [`UnsignedTransaction`] when given a signature
    /// and a public key.
    pub fn to_signed_tx(
        self,
        pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
        signature: <S::CryptoSpec as CryptoSpec>::Signature,
    ) -> Transaction<S> {
        Transaction::new(
            pub_key,
            self.runtime_msg,
            signature,
            self.chain_id,
            self.max_priority_fee_bips,
            self.max_fee,
            self.gas_limit.clone(),
            self.nonce,
        )
    }
}

type RawTxHash = [u8; 32];

impl<S: Spec> From<Transaction<S>> for AuthenticatedTransactionData<S> {
    fn from(tx: Transaction<S>) -> Self {
        let pub_key = tx.pub_key().clone();

        let credential_id = pub_key.credential_id::<<S::CryptoSpec as CryptoSpec>::Hasher>();
        let default_address = Some((&pub_key).into());

        Self {
            credentials: Credentials::new(pub_key),
            default_address,
            credential_id,
            chain_id: tx.chain_id,
            max_priority_fee_bips: tx.max_priority_fee_bips,
            max_fee: tx.max_fee,
            gas_limit: tx.gas_limit,
            nonce: tx.nonce,
        }
    }
}

/// Holds the original credentials to authenticate the transaction.
/// For example, this could be a public key of the sender of the transaction.
#[derive(Clone, Debug, Default)]
pub struct Credentials {
    credentials: BTreeMap<core::any::TypeId, Arc<dyn core::any::Any>>,
}

impl Credentials {
    /// Creates a new [`Credentials`] from the provided credential.
    pub fn new<T>(credential: T) -> Self
    where
        T: core::any::Any,
    {
        let mut map: BTreeMap<std::any::TypeId, Arc<dyn core::any::Any>> = BTreeMap::new();
        map.insert(core::any::TypeId::of::<T>(), Arc::new(credential));
        Self { credentials: map }
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
/// This is the output of the `RuntimeAuthenticator`.
pub struct AuthenticatedTransactionData<S: Spec> {
    /// Credential identifier used to retrieve relevant rollup address.
    pub credential_id: CredentialId,
    /// Holds the original credentials to authenticate the transaction and
    /// provides information about which `Authenticator` was used to authenticate the transaction.
    pub credentials: Credentials,
    /// The default address of the signer.
    pub default_address: Option<S::Address>,
    /// The chain ID.
    pub chain_id: u64,
    /// The maximum priority fee that can be paid for this transaction expressed in bips.
    /// This priority fee is computed as a percentage of the total gas consumed by the transaction
    pub max_priority_fee_bips: PriorityFeeBips,
    /// The maximum fee that can be paid for this transaction expressed as a the gas token amount
    pub max_fee: u64,
    /// The estimated gas usage of the transaction
    pub gas_limit: Option<S::Gas>,
    /// The nonce.
    pub nonce: u64,
}

impl<S: Spec> AuthenticatedTransactionData<S> {
    /// Creates a new [`TxGasMeter`] from the transaction data.
    pub(crate) fn gas_meter(&self, gas_price: &<S::Gas as Gas>::Price) -> TxGasMeter<S::Gas> {
        // We compute the gas amount that the transaction should consume.
        let gas_to_consume = match &self.gas_limit {
            // If the user has provided a gas limit, we use the `gas_limit * gas_price` as the amount to consume (EIP-1559).
            Some(gas_limit) => {
                // We need to check the gas price in case the user has provided a gas limit.
                gas_limit.value(gas_price)
            }
            // If the user has not provided a gas limit, we use the `max_fee` as the amount to consume.
            None => self.max_fee,
        };

        TxGasMeter {
            remaining_funds: gas_to_consume,
            gas_price: gas_price.clone(),
            gas_used: S::Gas::zero(),
        }
    }
}

pub struct AuthenticatedTransactionAndRawHash<S: Spec> {
    /// Hash of raw bytes.
    pub raw_tx_hash: RawTxHash,
    pub authenticated_tx: AuthenticatedTransactionData<S>,
}

/// A gas meter for transaction execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TxGasMeter<GU>
where
    GU: Gas,
{
    remaining_funds: u64,
    gas_price: GU::Price,
    gas_used: GU,
}

impl<GU> GasMeter<GU> for TxGasMeter<GU>
where
    GU: Gas,
{
    /// Returns the total gas incurred.
    fn gas_used(&self) -> &GU {
        &self.gas_used
    }

    /// Deducts the provided gas unit from the remaining funds, computing the scalar value of the
    /// funds from the price of the instance.
    fn charge_gas(&mut self, gas: &GU) -> Result<(), anyhow::Error> {
        // Check that there's enough gas to cover the cost before mutating the gas_used counter.
        // This ensures that in the corner case where...
        //  - User wants to do expensive operation
        //  - User does not have enough gas left
        // ... the check fails and the user does not lose any gas - which is what we want
        // since the operation won't be performed.
        //
        // This also ensures that the `gas_used` stays in sync with the `remaining_funds` counter.
        let funds_to_charge = gas.value(&self.gas_price);
        let remaining_funds = self.remaining_funds;
        self.remaining_funds = remaining_funds
            .checked_sub(funds_to_charge)
            .ok_or_else(|| anyhow::anyhow!(
                "Insufficient funds to charge gas. Required {funds_to_charge}, remaining {remaining_funds}"
            ))?;

        self.gas_used.combine(gas);

        Ok(())
    }

    /// Returns the gas price.
    fn gas_price(&self) -> &GU::Price {
        &self.gas_price
    }

    fn remaining_funds(&self) -> u64 {
        self.remaining_funds
    }
}

impl<GU> TxGasMeter<GU>
where
    GU: Gas,
{
    /// Returns a gas meter which does not charge for gas.
    pub(crate) fn unmetered() -> Self {
        Self {
            remaining_funds: u64::MAX,
            gas_price: GU::Price::ZEROED,
            gas_used: GU::ZEROED,
        }
    }
}
