use std::fmt::Debug;

use anyhow::Result;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{CallResponse, Context, DaSpec, EventEmitter, Spec, StateMap, TxState};

use crate::{prefix_from_address_with_parent, Event, PayeePolicy, Paymaster, PaymasterPolicy};

#[allow(type_alias_bounds)]
pub type PayeePolicyList<S: Spec> = Vec<(S::Address, PayeePolicy<S>)>;

/// This call messages for interacting with
/// the `Paymaster` module.
/// The `derive` for [`schemars::JsonSchema`] is a requirement of
/// [`sov_modules_api::ModuleCallJsonSchema`].
#[cfg_attr(
    feature = "native",
    derive(schemars::JsonSchema),
    schemars(bound = "S: Spec", rename = "CallMessage")
)]
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    Debug,
    PartialEq,
    Clone,
    serde::Serialize,
    serde::Deserialize,
    UniversalWallet,
)]
#[serde(bound = "S: Spec", rename_all = "snake_case")]
pub enum CallMessage<S: Spec> {
    RegisterPaymaster {
        policy: PaymasterPolicy<S, PayeePolicyList<S>>,
    },
    SetPayerForSequencer {
        payer: S::Address,
    },
    // TODO: Add/remove/update payees from policy
    // TODO: Add/remove sequencers from policy
    // TODO: Update default policy
    // TODO: Add/remove authorized updaters
    // <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1618>
}

impl<S: Spec> Paymaster<S> {
    /// Registers a new paymaster with the given policy. If the paymaster is willing to pay for txs from the
    /// current sequencer, then the paymaster is set as the payer for this sequencer.
    pub(crate) fn register_paymaster(
        &self,
        policy: PaymasterPolicy<S, PayeePolicyList<S>>,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse> {
        self.do_registration(
            context.sender(),
            std::iter::once(context.sequencer_da_address()),
            policy,
            state,
        )
    }

    /// Registers a new paymaster with the given policy. Sets the provided sequencer addresses
    /// to use that paymaster, if the paymaster policy permits it.
    pub(crate) fn do_registration<'a>(
        &self,
        new_payer: &S::Address,
        sequencer_addresses_to_register: impl Iterator<Item = &'a <S::Da as DaSpec>::Address>,
        policy: PaymasterPolicy<S, PayeePolicyList<S>>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse> {
        // Convert the set of payee policies to a statemap
        let payee_policies = StateMap::new(prefix_from_address_with_parent(
            self.payers.prefix(),
            new_payer,
        ));
        for (address, policy) in policy.payees {
            payee_policies.set(&address, &policy, state)?;
        }

        // Store the paymaster policy
        let policy = PaymasterPolicy {
            authorized_sequencers: policy.authorized_sequencers,
            authorized_updaters: policy.authorized_updaters,
            default_payee_policy: policy.default_payee_policy,
            payees: payee_policies,
        };
        self.payers.set(new_payer, &policy, state)?;

        self.emit_event(
            state,
            Event::<S>::RegisteredPaymaster {
                address: new_payer.clone(),
            },
        );

        for sequencer in sequencer_addresses_to_register {
            if policy.authorized_sequencers.covers(sequencer) {
                self.sequencer_to_payer.set(sequencer, new_payer, state)?;

                self.emit_event(
                    state,
                    Event::<S>::SetPayerForSequencer {
                        payer: new_payer.clone(),
                        sequencer: sequencer.clone(),
                    },
                );
            } else {
                tracing::debug!(sequencer = %sequencer, payer = %new_payer, "Attempt to register sequencer to use paymaster failed because payer policy does not permit it.");
            }
        }

        Ok(CallResponse::default())
    }

    /// Sets the payer address for the sequencer who sends sequences this call message to the given address
    /// if that payer's policy allows it.
    pub(crate) fn set_payer_for_sequencer(
        &self,
        payer: S::Address,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse> {
        let policy = self
            .payers
            .get(&payer, state)?
            .ok_or(anyhow::anyhow!("Paymaster {} is not registered", payer))?;

        if !policy
            .authorized_sequencers
            .covers(context.sequencer_da_address())
        {
            return Err(anyhow::anyhow!(
                "Sequencer {} is not authorized to user paymaster {}",
                context.sequencer_da_address(),
                payer
            ));
        }
        self.sequencer_to_payer
            .set(context.sequencer_da_address(), context.sender(), state)?;

        self.emit_event(
            state,
            Event::<S>::SetPayerForSequencer {
                payer: context.sender().clone(),
                sequencer: context.sequencer_da_address().clone(),
            },
        );

        Ok(CallResponse::default())
    }
}
