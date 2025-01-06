use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sov_bank::ReserveGasError;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{DaSpec, Gas, GasArray, Spec};

use crate::call::SafeVec;
use crate::PayeePolicyList;

/// The policy that the paymaster applies to a particular rollup user.
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    Serialize,
    Deserialize,
    Debug,
    PartialEq,
    Eq,
    Clone,
    JsonSchema,
    UniversalWallet,
)]
#[serde(bound = "S: Spec", rename_all = "snake_case")]
#[schemars(bound = "S: Spec", rename = "PayeePolicy")]
pub enum PayeePolicy<S: Spec> {
    /// The paymaster pays the fees for a particular sender when the policy allows it...
    /// - If the policy specifies a `max_fee`, the transaction's max fee must be less than or equal to that value
    /// - if the policy specifies a `max_gas_price`, the current gas price must be less than or equal to that value
    /// - If the policy specifies a gas limit, the transaction must also specify a limit *and* that limit must be less than or equal to `gas_limit`.
    ///
    /// In all other cases, the sender pays their own fees.
    Allow {
        #[allow(missing_docs)]
        max_fee: Option<u64>,
        #[allow(missing_docs)]
        gas_limit: Option<S::Gas>,
        #[allow(missing_docs)]
        max_gas_price: Option<<S::Gas as Gas>::Price>,
    },
    /// The payer does not pay fees for any transaction using this policy.
    Deny,
}

impl<S: Spec> PayeePolicy<S> {
    /// Checks that the transaction's max fee, gas price, and gas limit are all within the policy's limits, if applicable.
    pub fn authorize_transaction(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
    ) -> Result<(), ReserveGasError<S>> {
        if !self.authorizes_max_fee(tx.max_fee) {
            tracing::debug!(allowed_max_fee = ?self.max_fee(), requested_max_fee = %tx.max_fee, "Paymaster policy denied transaction payment due to max fee");
            return Err(ReserveGasError::InsufficientBalanceToReserveGas);
        }
        if !self.authorizes_gas_price(gas_price) {
            tracing::debug!(max_allowed_gas_price = ?self.max_gas_price(), current_gas_price = %gas_price, "Paymaster policy denied transaction payment because the gas price was too high");
            return Err(ReserveGasError::CurrentGasPriceTooHigh);
        }
        if !self.authorizes_gas_limit(&tx.gas_limit) {
            tracing::debug!(max_gas_limit = ?self.max_gas_limit(), requested_gas_limit = ?tx.gas_limit, "Paymaster policy denied transaction payment because the gas limit was too high");
            return Err(ReserveGasError::MaxGasLimitExceeded);
        }
        Ok(())
    }

    /// Checks that the transaction's max fee is less than the policy's max fee, if applicable.
    pub fn authorizes_max_fee(&self, tx_max_fee: u64) -> bool {
        // Use `match` instead of `if let` to ensure exhaustive pattern
        match self {
            PayeePolicy::Allow { max_fee, .. } => {
                if let Some(max_fee) = max_fee {
                    tx_max_fee <= *max_fee
                } else {
                    true
                }
            }
            PayeePolicy::Deny => false,
        }
    }

    /// Checks that the transaction's gas price is less than the policy's max gas price, if applicable
    pub fn authorizes_gas_price(&self, current_gas_price: &<S::Gas as Gas>::Price) -> bool {
        // Use `match` instead of `if let` to ensure exhaustive pattern
        match self {
            PayeePolicy::Allow { max_gas_price, .. } => {
                if let Some(max_gas_price) = max_gas_price {
                    // Check that each dimension of the gas price is within the allowed range by subtracting the current price
                    // from the max allowed value and checking that the result is non-negative in each dimension.
                    max_gas_price.checked_sub(current_gas_price).is_some()
                } else {
                    true
                }
            }
            PayeePolicy::Deny => false,
        }
    }

    /// Checks that the transaction's gas limit is less than the policy's max gas limit, if applicable
    pub fn authorizes_gas_limit(&self, tx_gas_limit: &Option<S::Gas>) -> bool {
        // Use `match` instead of `if let` to ensure exhaustive pattern
        match self {
            PayeePolicy::Allow { gas_limit, .. } => {
                // If the policy specifies a gas limit, the transaction must also specify a limit *and* that limit must be less than or equal to `gas_limit`.
                if let Some(policy_gas_limit) = gas_limit {
                    let Some(tx_gas_limit) = tx_gas_limit else {
                        return false;
                    };
                    // Check that each dimension of the gas limit is within the allowed range by subtracting the tx gas limit
                    // from the policy limit and checking that the result is non-negative in each dimension.
                    policy_gas_limit.checked_sub(tx_gas_limit).is_some()
                } else {
                    true
                }
            }
            PayeePolicy::Deny => false,
        }
    }

    fn max_fee(&self) -> Option<u64> {
        match self {
            PayeePolicy::Allow { max_fee, .. } => *max_fee,
            PayeePolicy::Deny => None,
        }
    }

    fn max_gas_price(&self) -> Option<<S::Gas as Gas>::Price> {
        match self {
            PayeePolicy::Allow { max_gas_price, .. } => max_gas_price.clone(),
            PayeePolicy::Deny => None,
        }
    }

    fn max_gas_limit(&self) -> Option<S::Gas> {
        match self {
            PayeePolicy::Allow { gas_limit, .. } => gas_limit.clone(),
            PayeePolicy::Deny => None,
        }
    }
}

/// The set of sequencers authorized to use a payer.
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    Clone,
    JsonSchema,
    UniversalWallet,
)]
#[serde(bound = "Da: DaSpec", rename_all = "snake_case")]
#[schemars(bound = "Da: DaSpec", rename = "AuthorizedSequencers")]
pub enum AuthorizedSequencers<Da: DaSpec> {
    /// All sequencers are authorized to use this payer (according to its policy).
    All,
    /// Only the specified sequencers may use this payer.
    Some(SafeVec<Da::Address>),
}

impl<Da: DaSpec> AuthorizedSequencers<Da> {
    /// Returns true if and only if the sequencer address is authorized to use the payer.
    pub fn covers(&self, address: &Da::Address) -> bool {
        match self {
            AuthorizedSequencers::All => true,
            AuthorizedSequencers::Some(addresses) => addresses.contains(address),
        }
    }
}

/// An initial policy for a paymaster. This includes...
///  - A set of sequencers that can use the paymaster
///  - A set of users authorized to update this policy
///  - A default policy for accepting/rejecting gas requests
///  - Specific policies for accepting/rejecting gas requests from particular users
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    Serialize,
    Deserialize,
    Debug,
    PartialEq,
    Eq,
    Clone,
    JsonSchema,
    UniversalWallet,
)]
#[serde(bound = "S: Spec ")]
#[schemars(bound = "S: Spec", rename = "PaymasterPolicyInitializer")]
pub struct PaymasterPolicyInitializer<S: Spec> {
    /// Default payee policy for users that are not in the balances map.
    pub default_payee_policy: PayeePolicy<S>,

    /// A mapping from user address to the policy for that user.
    pub payees: PayeePolicyList<S>,

    /// Users who are authorized to update this policy.
    pub authorized_updaters: SafeVec<S::Address>,

    /// Sequencers who are authorized to use this payer.
    pub authorized_sequencers: AuthorizedSequencers<S::Da>,
}

/// The policy for a paymaster. This includes...
///  - The set of sequencers that can use the paymaster
///  - The set of users authorized to update this policy
///  - A default policy for accepting/rejecting gas requests
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    Serialize,
    Deserialize,
    Debug,
    PartialEq,
    Eq,
    Clone,
    JsonSchema,
    UniversalWallet,
)]
#[serde(bound = "S: Spec")]
#[schemars(bound = "S: Spec", rename = "PaymasterPolicy")]
pub struct PaymasterPolicy<S: Spec> {
    /// Default payee policy for users that are not in the balances map.
    pub default_payee_policy: PayeePolicy<S>,

    /// Users who are authorized to update this policy.
    pub authorized_updaters: SafeVec<S::Address>,

    /// Sequencers who are authorized to use this payer.
    pub authorized_sequencers: AuthorizedSequencers<S::Da>,
}
