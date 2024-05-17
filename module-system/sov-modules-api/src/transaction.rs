use core::fmt::Formatter;
use std::cmp::min;
use std::fmt::Display;

use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_macros::cycle_tracker;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sov_modules_macros::config_value;
#[cfg(feature = "native")]
pub use sov_rollup_interface::crypto::PrivateKey;
use sov_rollup_interface::crypto::{PublicKey, Signature as _};
use sov_rollup_interface::zk::CryptoSpec;

use crate::{CredentialId, Gas, GasArray, GasMeter, Spec};

const EXTEND_MESSAGE_LEN: usize = 4 * core::mem::size_of::<u64>();

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
    Debug, PartialEq, Eq, Clone, borsh::BorshDeserialize, borsh::BorshSerialize, serde::Serialize,
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
        let gas_limit_len = self
            .gas_limit
            .as_ref()
            .map(|m| 1 + 8 * m.as_slice().len())
            .unwrap_or(1);

        let mut serialized_tx =
            Vec::with_capacity(self.runtime_msg().len() + EXTEND_MESSAGE_LEN + gas_limit_len);

        serialized_tx.extend_from_slice(self.runtime_msg());
        serialized_tx.extend_from_slice(&self.chain_id.to_le_bytes());
        serialized_tx
            .extend_from_slice(&Into::<u64>::into(self.max_priority_fee_bips).to_le_bytes());
        serialized_tx.extend_from_slice(&self.max_fee.to_le_bytes());
        serialized_tx.extend_from_slice(&self.nonce.to_le_bytes());

        match &self.gas_limit {
            Some(m) => {
                serialized_tx.push(1);
                m.as_slice()
                    .iter()
                    .for_each(|m| serialized_tx.extend_from_slice(&m.to_le_bytes()));
            }
            None => {
                serialized_tx.push(0);
            }
        }

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
        mut message: Vec<u8>,
        chain_id: u64,
        max_priority_fee_bips: PriorityFeeBips,
        max_fee: u64,
        gas_limit: Option<S::Gas>,
        nonce: u64,
    ) -> Self {
        // Since we own the message already, try to add the serialized nonce in-place.
        // This lets us avoid a copy if the message vec has at least 8 bytes of extra capacity.
        let len = message.len();
        let gas_limit_len = gas_limit
            .as_ref()
            .map(|m| 1 + 8 * m.as_slice().len())
            .unwrap_or(1);

        // resizes once to avoid potential multiple realloc
        message.resize(len + EXTEND_MESSAGE_LEN + gas_limit_len, 0);

        message[len..len + 8].copy_from_slice(&chain_id.to_le_bytes());
        message[len + 8..len + 16]
            .copy_from_slice(&Into::<u64>::into(max_priority_fee_bips).to_le_bytes());
        message[len + 16..len + 24].copy_from_slice(&max_fee.to_le_bytes());
        message[len + 24..len + 32].copy_from_slice(&nonce.to_le_bytes());

        match gas_limit.as_ref() {
            Some(m) => {
                message[len + 32] = 1;
                m.as_slice().iter().enumerate().for_each(|(i, m)| {
                    let from = len + 33 + i * 8;
                    let to = len + 33 + (i + 1) * 8;
                    message[from..to].copy_from_slice(&m.to_le_bytes());
                });
            }
            None => {
                message[len + 32] = 0;
            }
        }

        let pub_key = priv_key.pub_key();
        let signature = priv_key.sign(&message);

        // Don't forget to truncate the message back to its original length!
        message.truncate(len);

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
}

/// An unsent transaction with the required data to be submitted to the DA layer
#[derive(Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(bound = "Tx: serde::Serialize + serde::de::DeserializeOwned")]
pub struct UnsignedTransaction<S: Spec, Tx>
where
    Tx: BorshSerialize + BorshDeserialize,
{
    /// The underlying transaction
    pub tx: Tx,
    /// The ID of the target chain
    pub chain_id: u64,
    /// The maximum priority fee that can be paid for this transaction expressed in bips.
    /// This priority fee is computed as a percentage of the total gas consumed by the transaction
    pub max_priority_fee_bips: PriorityFeeBips,
    /// The maximum fee that can be paid for this transaction expressed as a the gas token amount
    pub max_fee: u64,
    /// The estimated gas usage of the transaction
    /// This is an optional field that can be used to provide an estimate of the gas usage of the transaction
    /// across the different gas dimensions. If provided, this quantity will be used along
    /// with the current multi-dimensional gas price to compute the estimated transaction fee and compare it to the `max_fee`
    pub gas_limit: Option<S::Gas>,
}

impl<S: Spec, Tx> UnsignedTransaction<S, Tx>
where
    Tx: Serialize + DeserializeOwned + BorshSerialize + BorshDeserialize,
{
    pub const fn new(
        tx: Tx,
        chain_id: u64,
        max_priority_fee_bips: PriorityFeeBips,
        max_fee: u64,
        gas_limit: Option<S::Gas>,
    ) -> Self {
        Self {
            tx,
            chain_id,
            max_priority_fee_bips,
            max_fee,
            gas_limit,
        }
    }
}

type RawTxHash = [u8; 32];

impl<S: Spec> From<Transaction<S>> for AuthenticatedTransactionData<S> {
    fn from(tx: Transaction<S>) -> Self {
        let credential_id = tx
            .pub_key()
            .credential_id::<<S::CryptoSpec as CryptoSpec>::Hasher>();

        let default_address = Some(tx.pub_key().into());

        Self {
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

/// Transaction data that has been authenticated.
/// This is the output of the `RuntimeAuthenticator`.
pub struct AuthenticatedTransactionData<S: Spec> {
    pub credential_id: CredentialId,
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
    /// Builds a [`TransactionConsumption`] from the [`AuthenticatedTransactionData`] and the associated [`GasMeter`].
    /// This method consumes the [`GasMeter`] to ensure that the transaction reward is only computed once at the end of the transaction execution.
    pub fn transaction_reward(&self, gas_meter: TxGasMeter<S::Gas>) -> TransactionConsumption {
        // The base fee is the amount of gas consumed by the transaction execution.
        let base_fee = gas_meter.gas_used().value(gas_meter.gas_price());

        // We compute the `max_priority_fee_bips` by applying the `priority_fee_per_gas` to the consumed gas.
        let max_priority_fee_bips = self
            .max_priority_fee_bips
            .apply(base_fee)
            // if the computation overflows, we return the max fee - we always have `priority_fee <= tx.max_priority_fee_bips() <= tx.max_fee()`
            .unwrap_or(self.max_fee);

        // The tip is the minimum of the remaining gas allocated to the transaction and the maximum priority fee per gas.
        // We transfer the tip to the tip recipient address.
        let tip = min(max_priority_fee_bips, self.max_fee - base_fee);

        TransactionConsumption {
            base_fee,
            priority_fee: tip,
        }
    }

    /// Creates a new [`TxGasMeter`] from the transaction data.
    pub fn gas_meter(&self, gas_price: &<S::Gas as Gas>::Price) -> TxGasMeter<S::Gas> {
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

/// The format of the resources consumed by the transaction. The base fee and the priority fee are expressed as gas token amounts.
/// The [`TransactionConsumption`] data structure can only be built from the [`AuthenticatedTransactionData`] data structure and consumes the associated gas meter.
#[derive(PartialEq, Eq, Debug)]
pub struct TransactionConsumption {
    /// The base fee reward of the transaction expressed as a gas token amount.
    base_fee: u64,
    /// The priority fee reward of the transaction expressed as a gas token amount.
    priority_fee: u64,
}

impl TransactionConsumption {
    /// A zero consumption. Happens when the transaction is ignored (like in the case of a revert for the speculative execution mode).
    pub const ZERO: Self = Self {
        base_fee: 0,
        priority_fee: 0,
    };

    /// The base fee reward of the transaction expressed as a gas token amount.
    pub const fn base_fee(&self) -> u64 {
        self.base_fee
    }

    /// The priority fee reward of the transaction expressed as a gas token amount.
    pub const fn priority_fee(&self) -> u64 {
        self.priority_fee
    }

    pub const fn total_consumption(&self) -> u64 {
        self.base_fee + self.priority_fee
    }
}

impl Display for TransactionConsumption {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TransactionConsumption {{ base_fee: {}, priority_fee: {} }}",
            self.base_fee, self.priority_fee
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
pub struct SequencerReward(u64);

impl SequencerReward {
    /// Returns a zero sequencer reward. This can be used to initialize an accumulator to build a sequencer reward.
    pub const ZERO: Self = Self(0);

    /// Adds another reward to this reward. Consumes the other reward.
    /// If the result overflows, we saturate.
    pub fn accumulate(&mut self, other: Self) {
        self.0 = self.0.saturating_add(other.0);
    }
}

impl From<TransactionConsumption> for SequencerReward {
    fn from(value: TransactionConsumption) -> Self {
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

/// A gas meter for transaction execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TxGasMeter<GU>
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
}

impl<GU> TxGasMeter<GU>
where
    GU: Gas,
{
    /// Returns the remaining gas funds.
    pub const fn remaining_funds(&self) -> u64 {
        self.remaining_funds
    }

    /// Returns a gas meter which does not charge for gas.
    /// TODO(@theochap) `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/678>`: remove this once we have a way to call rpc methods using the `StateCheckpoint`.
    pub fn unmetered() -> Self {
        Self {
            remaining_funds: u64::MAX,
            gas_price: GU::Price::ZEROED,
            gas_used: GU::ZEROED,
        }
    }
}

#[cfg(test)]
mod tests {
    use sov_mock_zkvm::MockZkVerifier;

    use super::{AuthenticatedTransactionData, PriorityFeeBips};
    use crate::default_spec::DefaultSpec;
    use crate::transaction::{SequencerReward, TransactionConsumption, TxGasMeter};
    use crate::{CredentialId, GasArray, GasMeter, GasPrice, GasUnit};

    /// Consume all the gas in the gas meter, so the transaction reward is the same as the base fee and there is no priority fee.
    #[test]
    fn test_compute_transaction_reward_consume_all_gas() {
        const REMAINING_FUNDS: u64 = 100;

        let tx_data = AuthenticatedTransactionData::<DefaultSpec<MockZkVerifier, MockZkVerifier>> {
            credential_id: CredentialId([1; 32]),
            default_address: None,
            chain_id: 0,
            max_priority_fee_bips: PriorityFeeBips::from_percentage(10),
            max_fee: REMAINING_FUNDS,
            gas_limit: None,
            nonce: 0,
        };

        let mut gas_meter: TxGasMeter<GasUnit<2>> =
            tx_data.gas_meter(&GasPrice::from_slice(&[1; 2]));

        gas_meter
            .charge_gas(&GasArray::from_slice(&[REMAINING_FUNDS / 2; 2]))
            .expect("There should be enough gas to charge locked in the meter.");

        let tx_reward = tx_data.transaction_reward(gas_meter);

        assert_eq!(
            tx_reward,
            TransactionConsumption {
                base_fee: 100,
                priority_fee: 0
            }
        );
    }

    /// Consume half of the gas in the gas meter, so the transaction reward is half of the initial funds and there is a maximum priority fee (100%).
    #[test]
    fn test_compute_transaction_reward_consume_not_all_gas() {
        const REMAINING_FUNDS: u64 = 100;

        let tx_data = AuthenticatedTransactionData::<DefaultSpec<MockZkVerifier, MockZkVerifier>> {
            credential_id: CredentialId([1; 32]),
            default_address: None,
            chain_id: 0,
            max_priority_fee_bips: PriorityFeeBips::from_percentage(100),
            max_fee: REMAINING_FUNDS,
            gas_limit: None,
            nonce: 0,
        };

        let mut gas_meter: TxGasMeter<GasUnit<2>> =
            tx_data.gas_meter(&GasPrice::from_slice(&[1; 2]));

        gas_meter
            .charge_gas(&GasArray::from_slice(&[REMAINING_FUNDS / 4; 2]))
            .expect("There should be enough gas to charge locked in the meter.");

        let tx_reward = tx_data.transaction_reward(gas_meter);

        assert_eq!(
            tx_reward,
            TransactionConsumption {
                base_fee: 50,
                priority_fee: 50
            }
        );
    }

    #[test]
    fn test_display_transaction_reward() {
        let tx_reward = TransactionConsumption {
            base_fee: 100,
            priority_fee: 50,
        };

        assert_eq!(
            format!("{}", tx_reward),
            "TransactionConsumption { base_fee: 100, priority_fee: 50 }"
        );
    }

    #[test]
    fn test_display_sequencer_reward() {
        let seq_reward = SequencerReward(100);

        assert_eq!(format!("{}", seq_reward), "SequencerReward(100)");
    }
}
