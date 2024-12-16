#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod call;
mod event;
mod genesis;
mod policies;
pub use call::*;
pub use event::Event;
pub use genesis::*;
pub use policies::*;
use sov_bank::ReserveGasError;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{
    Context, DaSpec, Error, Gas, GenesisState, InfallibleStateAccessor, InnerEnumVariant, Module,
    ModuleId, ModuleInfo, ModuleRestApi, Spec, StateMap, TxState,
};
use sov_state::BcsCodec;

pub(crate) type C = BcsCodec;

#[allow(type_alias_bounds)]
type PayeePolicyMap<S: Spec> = StateMap<S::Address, PayeePolicy<S>, C>;

/// The `Paymaster` module allows a third party to gas on behalf of a user.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
#[module_info(sequencer_safety = "is_safe_for_sequencer")]
pub struct Paymaster<S: Spec> {
    /// Id of the module.
    #[id]
    pub id: ModuleId,

    /// A mapping from paymaster addresses to their policies.
    #[state]
    pub payers: StateMap<S::Address, PaymasterPolicy<S, PayeePolicyMap<S>>, C>,

    /// Maps the sequencer to the appropriate gas payer.
    #[state]
    pub sequencer_to_payer: StateMap<<S::Da as DaSpec>::Address, S::Address, C>,

    /// Reference to the Bank module.
    #[module]
    pub(crate) bank: sov_bank::Bank<S>,
}

fn is_safe_for_sequencer<S: Spec>(
    _module: &Paymaster<S>,
    call: InnerEnumVariant<'_>,
    sequencer_address: &<S::Da as DaSpec>::Address,
) -> bool {
    if let Some(call) = call.inner().downcast_ref::<CallMessage<S>>() {
        // Updates are unsafe if they could change the payer registered for this sequencer
        match call {
            CallMessage::RegisterPaymaster { policy } => {
                // If a policy doesn't include this sequencer, we're fine
                !policy.authorized_sequencers.covers(sequencer_address)
            }
            // A set payer message is always dangerous
            //
            // Similarly, any policy update could change this sequencer's payer since the sequencer may already
            // be an authorized sequencer on that policy.
            // TODO: Change the update rules so that the sequencer is changed only if *newly* whitlisted.
            // Then, update this sequencer safety implementation
            CallMessage::SetPayerForSequencer { .. } | CallMessage::UpdatePolicy { .. } => false,
        }
    } else {
        // Calls to other modules are safe as far as we're concerned
        true
    }
}

impl<S: Spec> Module for Paymaster<S> {
    type Spec = S;

    type Config = genesis::PaymasterConfig<S>;

    type CallMessage = CallMessage<S>;

    type Event = Event<S>;

    fn genesis(
        &self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        _validity_condition: &<<S as Spec>::Da as DaSpec>::ValidityCondition,
        config: &Self::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<(), Error> {
        // The initialization logic
        Ok(self.init_module(config, state)?)
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> Result<(), Error> {
        match msg {
            CallMessage::RegisterPaymaster { policy } => {
                self.register_paymaster(policy, context, state)?;
            }
            CallMessage::SetPayerForSequencer { payer } => {
                self.set_payer_for_sequencer(payer, context, state)?;
            }
            CallMessage::UpdatePolicy { payer, update } => {
                self.update_policy_if_authorized(update, context, &payer, state)?;
            }
        }
        Ok(())
    }
}

impl<S: Spec> Paymaster<S> {
    /// Get the payer's policy pertaining to the current `context.sender()`
    fn get_payee_policy_if_sequencer_authorized(
        &self,
        payer: &S::Address,
        context: &Context<S>,
        state: &mut impl InfallibleStateAccessor,
    ) -> Option<PayeePolicy<S>> {
        let policy = self.payers.get(payer, state).unwrap_infallible()?;
        if !policy
            .authorized_sequencers
            .covers(context.sequencer_da_address())
        {
            return None;
        }
        Some(
            policy
                .payees
                .get(context.sender(), state)
                .unwrap_infallible()
                .unwrap_or(policy.default_payee_policy),
        )
    }

    /// Try to reserve gas for the transaction using the payer's balance if possible and falling back to the sender's balance if not.
    pub fn try_reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        context: &mut Context<S>,
        state: &mut impl InfallibleStateAccessor,
    ) -> Result<(), ReserveGasError<S>> {
        if let Some(payer) = self.gas_from_paymaster(tx, gas_price, context, state) {
            context.set_gas_refund_recipient(payer);
            return Ok(());
        }
        // If the paymaster will not pay for whatever reason, the user pays.
        // This prevents someone from censoring transactions by setting overly strict payer policies which cause
        // them to be rejected.
        tracing::debug!("Falling back to user balance to reserve gas");
        self.bank
            .reserve_gas(tx, gas_price, context.sender(), state)?;
        context.set_gas_refund_recipient(context.sender().clone());
        Ok(())
    }

    /// Reserves the entire available gas amount from the paymaster if the paymaster's policy permits it
    /// and it has sufficient balance. Otherwise, reserves no gas and returns None.
    fn gas_from_paymaster(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        context: &Context<S>,
        state: &mut impl InfallibleStateAccessor,
    ) -> Option<S::Address> {
        let payer = self
            .sequencer_to_payer
            .get(context.sequencer_da_address(), state)
            .unwrap_infallible()?;

        // If this sequencer acts as a paymaster, it reserves the gas for the transaction.
        if let Some(payee_policy) =
            self.get_payee_policy_if_sequencer_authorized(&payer, context, state)
        {
            // If the paymaster pays for the gas, it also needs to get the refund
            match self.try_purchase_paymaster_gas(tx, gas_price, &payer, &payee_policy, state) {
                Ok(()) => {
                    return Some(payer);
                }
                Err(e) => {
                    tracing::debug!(reason = %e, "Failed to pay gas using paymaster");
                }
            }
        } else {
            // If the sequencer isn't authorized for this payer, our map is stale. Remove the stale entry
            self.sequencer_to_payer
                .remove(context.sequencer_da_address(), state)
                .unwrap_infallible();
            tracing::debug!(
                sequencer = %context.sequencer_da_address(),
                attempted_payer = %payer,
                "Sequencer is not authorized for payer. Removing sequencer_to_payer entry.",
            );
        }
        None
    }

    /// Purchases the gas for the transaction using the payer's balance if possible.
    fn try_purchase_paymaster_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        payer: &S::Address,
        policy: &PayeePolicy<S>,
        state: &mut impl InfallibleStateAccessor,
    ) -> Result<(), ReserveGasError<S>> {
        policy.authorize_transaction(tx, gas_price)?;
        self.bank.reserve_gas(tx, gas_price, payer, state)
    }
}
