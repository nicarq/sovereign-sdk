#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

/// Call methods for the module
mod call;
/// Methods used to instantiate the module
mod genesis;

pub use call::*;
pub use genesis::*;

#[cfg(test)]
mod tests;

#[cfg(feature = "native")]
mod rpc;

mod event;
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "native")]
pub use rpc::*;
use sov_bank::{Amount, TokenId};
use sov_modules_api::hooks::TransitionHeight;
use sov_modules_api::{Context, DaSpec, Error, ModuleInfo, Spec, WorkingSet, Zkvm};
use sov_state::codec::BcsCodec;

use crate::event::Event;

/// The information about an attender's unbonding
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, PartialEq, Eq)]
pub struct UnbondingInfo {
    /// The height at which an attester started unbonding
    pub unbonding_initiated_height: TransitionHeight,
    /// The number of tokens that the attester may withdraw
    pub amount: Amount,
}

/// A new module:
/// - Must derive `ModuleInfo`
/// - Must contain `[address]` field
/// - Can contain any number of ` #[state]` or `[module]` fields
#[derive(ModuleInfo)]
pub struct AttesterIncentives<S, Da>
where
    S: Spec,
    Da: DaSpec,
{
    /// Address of the module.
    #[address]
    pub address: S::Address,

    /// The amount of time it takes to a light client to be confident
    /// that an attested state transition won't be challenged. Measured in
    /// number of slots.
    #[state]
    pub rollup_finality_period: sov_modules_api::StateValue<TransitionHeight>,

    /// The ID of the token used for bonding provers
    #[state]
    pub bonding_token_id: sov_modules_api::StateValue<TokenId>,

    /// The address of the account holding the reward token supply
    #[state]
    pub reward_token_supply_address: sov_modules_api::StateValue<S::Address>,

    /// The code commitment to be used for verifying proofs
    #[state]
    pub commitment_to_allowed_challenge_method:
        sov_modules_api::StateValue<<S::InnerZkvm as Zkvm>::CodeCommitment, BcsCodec>,

    /// The set of bonded attesters and their bonded amount.
    #[state]
    pub bonded_attesters: sov_modules_api::StateMap<S::Address, Amount>,

    /// The set of unbonding attesters, and the unbonding information (ie the
    /// height of the chain where they started the unbonding and their associated bond).
    #[state]
    pub unbonding_attesters: sov_modules_api::StateMap<S::Address, UnbondingInfo>,

    /// The current maximum attestation height
    #[state]
    pub maximum_attested_height: sov_modules_api::StateValue<TransitionHeight>,

    /// Challengers now challenge a transition and not a specific attestation
    /// Mapping from a transition number to the associated reward value.
    /// This mapping is populated when the attestations are processed by the rollup
    #[state]
    pub bad_transition_pool: sov_modules_api::StateMap<TransitionHeight, Amount>,

    /// The set of bonded challengers and their bonded amount.
    #[state]
    pub bonded_challengers: sov_modules_api::StateMap<S::Address, Amount>,

    /// The minimum bond for an attester to be eligble
    #[state]
    pub minimum_attester_bond: sov_modules_api::StateValue<Amount>,

    /// The minimum bond for an attester to be eligble
    #[state]
    pub minimum_challenger_bond: sov_modules_api::StateValue<Amount>,

    /// The height of the most recent block which light clients know to be finalized
    #[state]
    pub light_client_finalized_height: sov_modules_api::StateValue<TransitionHeight>,

    /// Reference to the Bank module.
    #[module]
    pub(crate) bank: sov_bank::Bank<S>,

    /// Reference to the chain state module, used to check the initial hashes of the state transition.
    #[kernel_module]
    pub(crate) chain_state: sov_chain_state::ChainState<S, Da>,
}

impl<S, Da> sov_modules_api::Module for AttesterIncentives<S, Da>
where
    S: sov_modules_api::Spec,
    Da: DaSpec,
{
    type Spec = S;

    type Config = AttesterIncentivesConfig<S, Da>;

    type CallMessage = call::CallMessage<S, Da>;

    type Event = Event<S>;

    fn genesis(&self, config: &Self::Config, working_set: &mut WorkingSet<S>) -> Result<(), Error> {
        // The initialization logic
        Ok(self.init_module(config, working_set)?)
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<sov_modules_api::CallResponse, Error> {
        match msg {
            call::CallMessage::BondAttester(bond_amount) => self
                .bond_user_helper(bond_amount, context.sender(), Role::Attester, working_set)
                .map_err(|err| err.into()),
            call::CallMessage::BeginUnbondingAttester => self
                .begin_unbond_attester(context, working_set)
                .map_err(|error| error.into()),

            call::CallMessage::EndUnbondingAttester => self
                .end_unbond_attester(context, working_set)
                .map_err(|error| error.into()),
            call::CallMessage::BondChallenger(bond_amount) => self
                .bond_user_helper(bond_amount, context.sender(), Role::Challenger, working_set)
                .map_err(|err| err.into()),
            call::CallMessage::UnbondChallenger => self.unbond_challenger(context, working_set),
            call::CallMessage::ProcessAttestation(attestation) => self
                .process_attestation(context, attestation, working_set)
                .map_err(|error| error.into()),

            call::CallMessage::ProcessChallenge(proof, transition) => self
                .process_challenge(context, &proof, &transition, working_set)
                .map_err(|error| error.into()),
        }
        .map_err(|e| e.into())
    }
}
