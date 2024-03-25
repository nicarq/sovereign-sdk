#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod call;
mod event;
mod genesis;

#[cfg(test)]
mod tests;

#[cfg(feature = "native")]
mod rpc;

use anyhow::bail;
use borsh::{BorshDeserialize, BorshSerialize};
pub use call::*;
pub use genesis::*;
/// The response type used by RPC queries.
#[cfg(feature = "native")]
pub use rpc::*;
use serde::{Deserialize, Serialize};
use sov_bank::TokenId;
use sov_modules_api::hooks::TransitionHeight;
use sov_modules_api::{Context, DaSpec, Error, ModuleInfo, Spec, WorkingSet, Zkvm};
use sov_state::codec::BcsCodec;

use crate::event::Event;

/// This type alias represents the amount of tokens. This is consistent with the representation
/// used in [`AttesterIncentives`].
type Amount = u64;

#[derive(Debug, Clone, PartialEq, Eq, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
/// The burn rate of the prover's reward. We need to burn some of it to avoid the system participants to
/// be incentivized to prove and submit empty blocks.
pub struct BurnRate(u64);

impl BurnRate {
    /// Creates a new burn rate. This function is only called at genesis.
    pub fn new(burn_rate: u64) -> Result<Self, anyhow::Error> {
        // We can panic here since the burn rate is a constant defined at genesis
        if burn_rate > 100 {
            bail!("Burn rate must be less than or equal to 100");
        }

        Ok(Self(burn_rate))
    }

    /// Applies the burn rate to the given amount.
    pub(crate) fn apply(&self, amount: Amount) -> Amount {
        amount * (100 - self.0) / 100
    }
}

/// A new module:
/// - Must derive `ModuleInfo`
/// - Must contain `[address]` field
/// - Can contain any number of ` #[state]` or `[module]` fields
#[cfg_attr(feature = "native", derive(sov_modules_api::ModuleCallJsonSchema))]
#[derive(ModuleInfo)]
pub struct ProverIncentives<S: Spec, Da: DaSpec> {
    /// Address of the module.
    #[address]
    pub address: S::Address,

    /// The address of the account holding the reward token supply
    #[state]
    pub reward_token_supply_address: sov_modules_api::StateValue<S::Address>,

    /// The ID of the token used for bonding provers
    #[state]
    pub bonding_token_id: sov_modules_api::StateValue<TokenId>,

    /// The code commitment to be used for verifying proofs
    #[state]
    pub commitment_of_allowed_verifier_method:
        sov_modules_api::StateValue<<S::Zkvm as Zkvm>::CodeCommitment, BcsCodec>,

    /// The set of registered provers and their bonded amount.
    #[state]
    pub bonded_provers: sov_modules_api::StateMap<S::Address, Amount>,

    /// The minimum bond for a prover to be eligible for onchain verification
    #[state]
    pub minimum_bond: sov_modules_api::StateValue<Amount>,

    /// The burn rate of the reward price for the provers.
    /// The burn rate is a percentage of the base fee that is burned - this prevents provers from proving empty blocks.
    /// This is a constant defined at genesis for now.
    #[state]
    pub reward_burn_rate: sov_modules_api::StateValue<BurnRate>,

    /// The highest slot height for which the reward has been claimed. The next proofs should claim the next slot height.
    #[state]
    pub last_claimed_reward: sov_modules_api::StateValue<TransitionHeight>,

    /// A penalty for provers who submit a proof for transitions that were already proven
    #[state]
    pub proving_penalty: sov_modules_api::StateValue<Amount>,

    /// Reference to the Bank module.
    #[module]
    pub(crate) bank: sov_bank::Bank<S>,

    /// Reference to the Chain state module. Used to check the proof inputs
    #[kernel_module]
    pub(crate) chain_state: sov_chain_state::ChainState<S, Da>,
}

impl<S: Spec, Da: DaSpec> sov_modules_api::Module for ProverIncentives<S, Da> {
    type Spec = S;

    type Config = ProverIncentivesConfig<S>;

    type CallMessage = call::CallMessage;

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
            call::CallMessage::BondProver(bond_amount) => {
                self.bond_prover(bond_amount, context, working_set)
            }
            call::CallMessage::UnbondProver => self.unbond_prover(context, working_set),
            call::CallMessage::VerifyProof(proof) => {
                self.process_proof(&proof, context, working_set)
            }
        }
        .map_err(|e| Error::ModuleError(e.into()))
    }
}
