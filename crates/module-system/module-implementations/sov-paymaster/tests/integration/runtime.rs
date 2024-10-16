use sov_bank::IntoPayable;
use sov_modules_api::capabilities::{GasEnforcer, TryReserveGasError};
use sov_modules_api::transaction::{AuthenticatedTransactionData, ProverRewards, RemainingFunds};
use sov_modules_api::{Context, DaSpec, Gas, ModuleInfo, Spec, TxScratchpad};
use sov_paymaster::Paymaster;
use sov_test_utils::generate_runtime;
use sov_test_utils::runtime::genesis::optimistic::MinimalOptimisticGenesisConfig;
use sov_test_utils::runtime::{AttesterIncentives, SequencerRegistry, ValueSetter};

generate_runtime! {
    name: PaymasterRuntime,
    modules: [paymaster: Paymaster<S>, value_setter: ValueSetter<S>],
    operating_mode: sov_modules_api::runtime::OperatingMode::Optimistic,
    minimal_genesis_config_type: MinimalOptimisticGenesisConfig<S>,
    impl_hooks: [SlotHooks, FinalizeHook, ApplyBatchHooks, TxHooks],
    gas_enforcer_override: gas_enforcer,
    runtime_trait_impl_bounds: [],
    kernel_type: sov_kernels::basic::BasicKernel<'a, S>
}

pub struct PaymasterGasEnforcer<'a, S: Spec> {
    bank: &'a sov_bank::Bank<S>,
    paymaster: &'a Paymaster<S>,
    sequencer_registry: &'a SequencerRegistry<S>,
    attester_incentives: &'a AttesterIncentives<S>,
}

impl<'a, S: Spec> From<&'a PaymasterRuntime<S>> for PaymasterGasEnforcer<'a, S> {
    fn from(runtime: &'a PaymasterRuntime<S>) -> Self {
        Self {
            bank: &runtime.bank,
            paymaster: &runtime.paymaster,
            sequencer_registry: &runtime.sequencer_registry,
            attester_incentives: &runtime.attester_incentives,
        }
    }
}

impl<S: Spec> PaymasterRuntime<S> {
    fn gas_enforcer(&self) -> PaymasterGasEnforcer<'_, S> {
        self.into()
    }
}

impl<'a, S: Spec> GasEnforcer<S> for PaymasterGasEnforcer<'a, S> {
    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        context: &mut Context<S>,
        state: &mut TxScratchpad<S::Storage>,
    ) -> Result<(), TryReserveGasError> {
        self.paymaster
            .try_reserve_gas(tx, gas_price, context, state)
            .map_err(Into::into)
    }

    fn try_reserve_gas_for_proof(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        sender: &S::Address,
        state: &mut TxScratchpad<S::Storage>,
    ) -> Result<(), TryReserveGasError> {
        self.bank
            .reserve_gas(tx, gas_price, sender, state)
            .map_err(Into::into)
    }

    fn reward_prover(
        &self,
        prover_rewards: &ProverRewards,
        tx_state: &mut TxScratchpad<S::Storage>,
    ) {
        let rewarded_module = self.attester_incentives.id().to_payable();

        self.bank
            .reward_prover(&rewarded_module, prover_rewards, tx_state);
    }

    fn refund_remaining_gas(
        &self,
        sender: &S::Address,
        remaining_funds: &RemainingFunds,
        tx_state: &mut TxScratchpad<S::Storage>,
    ) {
        self.bank
            .refund_remaining_gas(sender, remaining_funds, tx_state);
    }

    fn transfer_authentication_cost_from_sequencer_to_prover(
        &self,
        amount: u64,
        sequencer: &<S::Da as DaSpec>::Address,
        tx_state: &mut TxScratchpad<S::Storage>,
    ) {
        let rewarded_module = self.attester_incentives.id().to_payable();
        self.sequencer_registry
            .remove_part_of_the_stake(sequencer, rewarded_module, amount, tx_state)
            .unwrap_or_else(|e| panic!("Unable to remove the sequencer's stake: {}", e));
    }

    fn transfer_authentication_cost_from_user_to_sequencer(
        &self,
        amount: u64,
        user: &S::Address,
        sequencer: &<S::Da as DaSpec>::Address,
        tx_state: &mut TxScratchpad<S::Storage>,
    ) {
        self.sequencer_registry
            .add_to_stake(user, sequencer, amount, tx_state)
            .unwrap_or_else(|e| panic!("Unable to increase the sequencer's stake {}", e));
    }
}
