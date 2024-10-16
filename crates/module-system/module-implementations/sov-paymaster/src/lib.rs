mod call;
mod event;
mod genesis;
mod policies;
#[cfg(feature = "native")]
mod query;
pub use call::CallMessage;
pub use event::Event;
pub use genesis::{PaymasterConfig, PaymasterSetup};
pub use policies::*;
#[cfg(feature = "native")]
pub use query::*;
use sov_bank::ReserveGasError;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{
    CallResponse, Context, DaSpec, Error, Gas, GenesisState, Module, ModuleId, ModuleInfo,
    ModuleRestApi, Spec, StateMap, TxScratchpad, TxState,
};
use sov_state::BcsCodec;

#[allow(type_alias_bounds)]
type PayeePolicyMap<S: Spec> = StateMap<S::Address, PayeePolicy<S>>;

/// The `Paymaster` module allows a third party to gas on behalf of a user.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct Paymaster<S: Spec> {
    /// Id of the module.
    #[id]
    pub id: ModuleId,

    /// A mapping from paymaster addresses to their policies.
    #[state]
    pub payers: StateMap<S::Address, PaymasterPolicy<S, PayeePolicyMap<S>>>,

    /// Maps the sequencer to the appropriate gas payer.
    #[state]
    pub sequencer_to_payer: StateMap<<S::Da as DaSpec>::Address, S::Address, BcsCodec>,

    /// Reference to the Bank module.
    #[module]
    pub(crate) bank: sov_bank::Bank<S>,
}

impl<S: Spec> Module for Paymaster<S> {
    type Spec = S;

    type Config = genesis::PaymasterConfig<S>;

    type CallMessage = CallMessage<S>;

    type Event = Event<S>;

    fn genesis(
        &self,
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
    ) -> Result<CallResponse, Error> {
        match msg {
            CallMessage::RegisterPaymaster { policy } => {
                self.register_paymaster(policy, context, state)?;
            }
            CallMessage::SetPayerForSequencer { payer } => {
                self.set_payer_for_sequencer(payer, context, state)?;
            }
        }
        Ok(CallResponse::default())
    }
}

impl<S: Spec> Paymaster<S> {
    /// Get the payer's policy pertaining to the current `context.sender()`
    fn get_payee_policy(
        &self,
        payer: &S::Address,
        context: &Context<S>,
        state: &mut TxScratchpad<S::Storage>,
    ) -> Option<PayeePolicy<S>> {
        let policy = self.payers.get(payer, state).unwrap_infallible()?;
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
        state: &mut TxScratchpad<S::Storage>,
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
        state: &mut TxScratchpad<S::Storage>,
    ) -> Option<S::Address> {
        let payer = self
            .sequencer_to_payer
            .get(context.sequencer_da_address(), state)
            .unwrap_infallible()?;

        // If this sequencer acts as a paymaster, it reserves the gas for the transaction.
        if let Some(payee_policy) = self.get_payee_policy(&payer, context, state) {
            // If the paymaster pays for the gas, it also needs to get the refund
            match self.try_purchase_paymaster_gas(tx, gas_price, &payer, &payee_policy, state) {
                Ok(()) => {
                    return Some(payer);
                }
                Err(e) => {
                    tracing::debug!(reason = %e, "Failed to pay gas using paymaster");
                }
            }
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
        state: &mut TxScratchpad<S::Storage>,
    ) -> Result<(), ReserveGasError<S>> {
        policy.authorize_transaction(tx, gas_price)?;
        self.bank.reserve_gas(tx, gas_price, payer, state)
    }
}

/// Creates a new prefix from an already existing prefix `parent_prefix` and a `token_id`
/// by extending the parent prefix.
// TODO: Separate prefix display from prefix creation
pub(crate) fn prefix_from_address_with_parent<A: std::fmt::Display>(
    parent_prefix: &sov_state::Prefix,
    address: &A,
) -> sov_state::Prefix {
    let mut prefix = parent_prefix.as_ref().to_vec();
    prefix.extend_from_slice(format!("{}", address).as_bytes());
    sov_state::Prefix::new(prefix)
}
