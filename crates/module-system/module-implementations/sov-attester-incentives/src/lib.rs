#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

/// Call methods for the module
mod call;
mod capabilities;
/// Methods used to instantiate the module
mod genesis;
mod helpers;
mod registration;
pub use call::*;
pub use genesis::*;

#[cfg(feature = "native")]
mod query;

mod event;
use borsh::{BorshDeserialize, BorshSerialize};
pub use capabilities::{ProcessAttestationErrors, ProcessChallengeErrors};
#[cfg(feature = "native")]
pub use query::*;
pub use registration::CustomError;
use sov_bank::{Amount, BurnRate};
use sov_chain_state::OperatingMode;
use sov_modules_api::hooks::TransitionHeight;
pub use sov_modules_api::optimistic::Attestation;
use sov_modules_api::{
    Context, DaSpec, Error, GenesisState, ModuleId, ModuleInfo, Spec, StateReader, TxState,
};
use sov_state::User;

pub use crate::event::Event;

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
#[derive(Clone, ModuleInfo)]
pub struct AttesterIncentives<S, Da>
where
    S: Spec,
    Da: DaSpec,
{
    /// Id of the module.
    #[id]
    pub id: ModuleId,

    /// The amount of time it takes to a light client to be confident
    /// that an attested state transition won't be challenged. Measured in
    /// number of slots.
    #[state]
    pub rollup_finality_period: sov_modules_api::StateValue<TransitionHeight>,

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
    /// This should always be above the maximum gas limit to avoid collusion.
    /// TODO(@theochap) `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/360>`: This bond should be express in gas units.
    #[state]
    pub minimum_attester_bond: sov_modules_api::StateValue<Amount>,

    /// The minimum bond for an attester to be eligble
    /// This should always be above the maximum gas limit to avoid collusion.
    /// TODO(@theochap) `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/360>`: This bond should be express in gas units.
    #[state]
    pub minimum_challenger_bond: sov_modules_api::StateValue<Amount>,

    /// The height of the most recent block which light clients know to be finalized
    #[state]
    pub light_client_finalized_height: sov_modules_api::StateValue<TransitionHeight>,

    /// The reward burn rate for the attester incentives module
    /// TODO(@theochap) `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/285>`: This should be a constant.
    #[state]
    pub reward_burn_rate: sov_modules_api::StateValue<BurnRate>,

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

    type Config = AttesterIncentivesConfig<S>;

    type CallMessage = call::CallMessage;

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
    ) -> Result<sov_modules_api::CallResponse, Error> {
        if !self.should_reward_fees(state) {
            return Err(anyhow::anyhow!(
                "Attester incentives call message received when operating in zk mode"
            )
            .into());
        }
        let res = match msg {
            call::CallMessage::RegisterAttester(bond_amount) => self
                .register_attester(bond_amount, context.sender(), state)
                .map_err(|err| err.into()),
            call::CallMessage::DepositAttester(amount) => self
                .deposit_attester(amount, context.sender(), state)
                .map_err(|err| err.into()),

            call::CallMessage::BeginExitAttester => self
                .begin_exit_attester(context, state)
                .map_err(|error| error.into()),
            call::CallMessage::ExitAttester => self
                .exit_attester(context, state)
                .map_err(|error| error.into()),
            call::CallMessage::RegisterChallenger(bond_amount) => self
                .register_challenger(bond_amount, context.sender(), state)
                .map_err(|err| err.into()),
            call::CallMessage::ExitChallenger => self.exit_challenger(context, state),
        }
        .map_err(|e| e.into());
        if let Err(ref err) = res {
            tracing::debug!("Attester incentives call reverted with error {}", err);
        } else {
            tracing::debug!("Attester incentives call succeeded!");
        }
        res
    }
}

impl<S: Spec, Da: DaSpec> AttesterIncentives<S, Da> {
    /// Returns a bool indicating if the [`AttesterIncentives`] module should be paid fees.
    pub fn should_reward_fees<Accessor: StateReader<User>>(&self, state: &mut Accessor) -> bool {
        self.chain_state
            .operating_mode(state)
            .expect("Operating mode retrieval should be infallible")
            == OperatingMode::Optimistic
    }
}
