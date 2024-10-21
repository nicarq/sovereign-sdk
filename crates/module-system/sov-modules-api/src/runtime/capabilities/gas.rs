use sov_rollup_interface::da::DaSpec;

use crate::transaction::{AuthenticatedTransactionData, ProverRewards, RemainingFunds};
use crate::{Context, Gas, GasSpec, Spec, TxScratchpad};

/// The error type returned by the [`GasEnforcer::try_reserve_gas`] method.
pub struct TryReserveGasError {
    /// The reason why it was not possible to reserve gas.
    pub reason: String,
}

/// Enforces gas limits and penalties for transactions.
pub trait GasEnforcer<S: Spec> {
    /// Maximum amount of gas the sequencer can pay for the tx execution. Typically this will be the sum
    /// of authentication (sig check) gas and process_tx_pre_exec_checks_gas.
    fn max_tx_check_costs(&self) -> <S as Spec>::Gas {
        <S as GasSpec>::max_tx_check_costs()
    }

    /// The gas used in the batch hooks.
    fn batch_hook_gas(&self) -> <S as Spec>::Gas {
        <S as GasSpec>::batch_hook_gas()
    }

    /// The gas used for the transaction pre-execution checks.
    /// For example nonce checks, context resolution etc..
    fn process_tx_pre_exec_checks_gas(&self) -> <S as Spec>::Gas {
        <S as GasSpec>::process_tx_pre_exec_checks_gas()
    }

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
        scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> Result<(), TryReserveGasError>;

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
        scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> Result<(), TryReserveGasError>;

    /// Rewards the prover
    /// This method should not fail.
    ///
    /// ## Correctness note
    /// TODO(@theochap): The rollup developper has to make sure to pre-allocate enough gas to prevent the
    /// transaction sender from underpaying for this operation.
    fn reward_prover(
        &self,
        prover_rewards: &ProverRewards,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    );

    /// Refunds any remaining gas to the payer after the transaction is processed.
    /// This method should not fail.
    ///
    /// ## Correctness note
    /// TODO(@theochap): The rollup developper has to make sure to pre-allocate enough gas to prevent the
    /// transaction sender from underpaying for this operation.
    fn refund_remaining_gas(
        &self,
        sender: &S::Address,
        remaining_funds: &RemainingFunds,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    );

    /// The sequencer refunds the prover for the authentication of the transactions.
    fn transfer_funds_from_sequencer_to_prover(
        &self,
        amount: u64,
        sequencer: &<<S as Spec>::Da as DaSpec>::Address,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> anyhow::Result<()>;

    /// The user refunds the sequencer for the authentication of its transaction.
    /// The caller should ensure that the user's balance will cover the cost; otherwise, the call will panic.
    fn transfer_authentication_cost_from_user_to_sequencer(
        &self,
        amount: u64,
        user: &S::Address,
        sequencer: &<<S as Spec>::Da as DaSpec>::Address,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    );
}
