use borsh::{BorshDeserialize, BorshSerialize};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sov_rollup_interface::da::DaSpec;

use crate::transaction::{AuthenticatedTransactionData, ProverRewards, RemainingFunds};
use crate::{Context, Gas, InfallibleStateAccessor, Spec, StateAccessor};

/// Enforces gas limits and penalties for transactions.
pub trait GasEnforcer<S: Spec> {
    /// Checks that the transaction has enough gas to be processed.
    ///
    /// ## Note
    /// This method has to reserve enough gas to cover the pre-execution checks cost of the transaction.
    /// If the transaction doesn't have enough gas to cover the pre-execution checks, the method should return an error.
    ///
    /// ## Behavior
    /// This function **should** charge the transaction sender for the gas locked in the transaction because his balance
    /// may change during the transaction execution.
    #[allow(clippy::result_large_err)]
    fn try_reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        ctx: &mut Context<S>,
        state: &mut impl StateAccessor,
    ) -> anyhow::Result<()>;

    /// Checks that the proof or attestation has enough gas to be processed.
    ///
    /// ## Note
    /// This method has to reserve enough gas to cover the pre-execution checks cost of the transaction.
    /// If the transaction doesn't have enough gas to cover the pre-execution checks, the method should return an error.
    ///
    /// ## Behavior
    /// This function **should** charge the transaction sender for the gas locked in the transaction because his balance
    /// may change during the transaction execution.
    #[allow(clippy::result_large_err)]
    fn try_reserve_gas_for_proof(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        sender: &S::Address,
        scratchpad: &mut impl StateAccessor,
    ) -> anyhow::Result<()>;

    /// Rewards the prover
    /// This method should not fail.
    ///
    /// ## Correctness note
    /// TODO(@theochap): The rollup developer has to make sure to pre-allocate enough gas to prevent the
    /// transaction sender from underpaying for this operation.
    fn reward_prover(
        &self,
        prover_rewards: &ProverRewards,
        tx_scratchpad: &mut impl InfallibleStateAccessor,
    );

    /// Refunds any remaining gas to the payer after the transaction is processed.
    /// This method should not fail.
    ///
    /// ## Correctness note
    /// TODO(@theochap): The rollup developer has to make sure to pre-allocate enough gas to prevent the
    /// transaction sender from underpaying for this operation.
    fn refund_remaining_gas(
        &self,
        sender: &S::Address,
        remaining_funds: &RemainingFunds,
        tx_scratchpad: &mut impl InfallibleStateAccessor,
    );

    /// The sequencer refunds the prover for the authentication of the transactions.
    /// This method is unmetered, so implementers MUST ensure that its cost is small.
    /// This is not difficult to do - in general, this  method should simply do one token transfer
    /// between known addresses.
    fn transfer_funds_from_sequencer_to_prover(
        &self,
        amount: u64,
        sequencer: &<<S as Spec>::Da as DaSpec>::Address,
        tx_scratchpad: &mut impl InfallibleStateAccessor,
    ) -> anyhow::Result<()>;

    /// The user refunds the sequencer for the authentication of its transaction.
    /// The caller should ensure that the user's balance will cover the cost; otherwise, the call will panic.
    fn transfer_authentication_cost_from_user_to_sequencer(
        &self,
        amount: u64,
        user: &S::Address,
        sequencer: &<<S as Spec>::Da as DaSpec>::Address,
        tx_scratchpad: &mut impl InfallibleStateAccessor,
    );
}

/// A structure that contains block gas information.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, BorshSerialize, BorshDeserialize)]
#[serde(bound = "GU: DeserializeOwned")]
pub struct BlockGasInfo<GU: Gas> {
    /// The gas limit of the block execution.
    /// This value is dynamically adjusted over time to account for the increase
    /// in proving/execution performance.
    gas_limit: GU,
    /// The gas used by the block execution.
    /// This value is set to zero at the beginning of the block execution (in the [`ChainState::synchronize_chain`] capability),
    /// and is populated once the block execution is complete.
    gas_used: GU,
    /// The base fee per gas used for the block execution. This value combined with the `gas_used`
    /// can be used to compute the total base fee (expressed in gas tokens) paid by the block execution.
    base_fee_per_gas: GU::Price,
}

impl<GU: Gas> BlockGasInfo<GU> {
    /// Creates a new [`BlockGasInfo`] with the provided gas limit and base fee per gas.
    /// The `gas_used` is set to zero.
    pub fn new(gas_limit: GU, base_fee_per_gas: GU::Price) -> Self {
        Self {
            gas_limit,
            gas_used: GU::zero(),
            base_fee_per_gas,
        }
    }

    /// Creates a new [`BlockGasInfo`] with the provided gas limit and base fee per gas.
    pub fn with_usage(gas_limit: GU, base_fee_per_gas: GU::Price, gas_used: GU) -> Self {
        Self {
            gas_limit,
            gas_used,
            base_fee_per_gas,
        }
    }

    /// Updates the gas used by the block execution.
    pub fn update_gas_used(&mut self, gas_used: GU) {
        self.gas_used = gas_used;
    }

    /// Returns the gas limit of the block execution.
    pub const fn gas_limit(&self) -> &GU {
        &self.gas_limit
    }

    /// Returns the gas used by the block execution.
    pub const fn gas_used(&self) -> &GU {
        &self.gas_used
    }

    /// Returns the base fee per gas used for the block execution.
    pub const fn base_fee_per_gas(&self) -> &GU::Price {
        &self.base_fee_per_gas
    }
}
