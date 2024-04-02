#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

/// Contains the call methods used by the module
mod call;
mod gas;
pub use gas::GasPriceState;
#[cfg(test)]
mod tests;
use sov_modules_api::{Spec, StateAccessor, StateReaderAndWriter};

mod genesis;
pub use genesis::*;

/// Hook implementation for the module
pub mod hooks;

#[cfg(feature = "test-utils")]
mod utils;

/// The query interface with the module
#[cfg(feature = "native")]
mod rpc;
use borsh::{BorshDeserialize, BorshSerialize};
use derivative::Derivative;
use serde::{Deserialize, Serialize};
use sov_modules_api::da::Time;
pub use sov_modules_api::hooks::TransitionHeight;
use sov_modules_api::namespaces::Kernel;
use sov_modules_api::{
    DaSpec, Error, Gas, KernelModule, KernelModuleInfo, ValidityConditionChecker, WorkingSet,
};
use sov_state::codec::BcsCodec;
use sov_state::storage::kernel_state::VersionReader;
use sov_state::storage::KernelWorkingSet;
use sov_state::Storage;

/// Type alias that contains the height of a given transition
pub type VirtualSlotNumber = u64;

#[derive(Derivative, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Debug)]
// We need to use derivative here because `Storage` doesn't implement `Eq` and `PartialEq`
#[derivative(
    PartialEq(bound = "S: Spec, Da: DaSpec"),
    Eq(bound = "S: Spec, Da: DaSpec")
)]
/// Structure that contains the information needed to represent a single state transition.
pub struct StateTransition<S: Spec, Da: DaSpec> {
    slot_hash: Da::SlotHash,
    post_state_root: <S::Storage as Storage>::Root,
    validity_condition: Da::ValidityCondition,
    gas_price: <S::Gas as Gas>::Price,
    gas_used: S::Gas,
}

impl<S: Spec, Da: DaSpec> StateTransition<S, Da> {
    /// Creates a new state transition. Only available for testing as we only want to create
    /// new state transitions from existing [`TransitionInProgress`].
    pub fn new(
        slot_hash: Da::SlotHash,
        post_state_root: <S::Storage as Storage>::Root,
        validity_condition: Da::ValidityCondition,
        gas_price: <S::Gas as Gas>::Price,
        gas_used: S::Gas,
    ) -> Self {
        Self {
            slot_hash,
            post_state_root,
            validity_condition,
            gas_price,
            gas_used,
        }
    }
}

impl<S: Spec, Da: DaSpec> StateTransition<S, Da> {
    /// Compare the transition block hash and state root with the provided input couple. If
    /// the pairs are equal, return [`true`].
    pub fn compare_hashes(
        &self,
        slot_hash: &Da::SlotHash,
        post_state_root: &<S::Storage as Storage>::Root,
    ) -> bool {
        self.slot_hash == *slot_hash && self.post_state_root == *post_state_root
    }

    /// Returns the post state root of a state transition
    pub fn post_state_root(&self) -> &<S::Storage as Storage>::Root {
        &self.post_state_root
    }

    /// Returns the slot hash of a state transition
    pub fn slot_hash(&self) -> &Da::SlotHash {
        &self.slot_hash
    }

    /// Returns the total gas used for the block execution
    pub const fn gas_used(&self) -> &S::Gas {
        &self.gas_used
    }

    /// Returns the gas price computed for the block execution
    pub const fn gas_price(&self) -> &<S::Gas as Gas>::Price {
        &self.gas_price
    }

    /// Returns the validity condition associated with the transition
    pub fn validity_condition(&self) -> &Da::ValidityCondition {
        &self.validity_condition
    }

    /// Checks the validity condition of a state transition
    pub fn validity_condition_check<Checker: ValidityConditionChecker<Da::ValidityCondition>>(
        &self,
        checker: &mut Checker,
    ) -> Result<(), <Checker as ValidityConditionChecker<Da::ValidityCondition>>::Error> {
        checker.check(&self.validity_condition)
    }
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
/// Represents a transition in progress for the rollup.
pub struct TransitionInProgress<S: Spec, Da: DaSpec> {
    slot_hash: Da::SlotHash,
    validity_condition: Da::ValidityCondition,
    gas_price: <S::Gas as Gas>::Price,
    gas_used: S::Gas,
}

impl<S: Spec, Da: DaSpec> TransitionInProgress<S, Da> {
    /// Creates a new transition in progress
    pub fn new(
        slot_hash: Da::SlotHash,
        validity_condition: Da::ValidityCondition,
        gas_price: <S::Gas as Gas>::Price,
        gas_used: S::Gas,
    ) -> Self {
        Self {
            slot_hash,
            validity_condition,
            gas_price,
            gas_used,
        }
    }

    /// Returns the gas price of the transition.
    pub const fn gas_price(&self) -> &<S::Gas as Gas>::Price {
        &self.gas_price
    }

    /// Returns the total gas used of the transition.
    pub const fn gas_used(&self) -> &S::Gas {
        &self.gas_used
    }

    /// Returns the block hash of the transition in progress
    pub const fn block_hash(&self) -> &Da::SlotHash {
        &self.slot_hash
    }
}

/// The chain state module definition. Contains the current state of the da layer.
#[derive(Clone, KernelModuleInfo)]
pub struct ChainState<S: Spec, Da: DaSpec> {
    /// Address of the module.
    #[address]
    address: S::Address,

    /// The height that should be loaded as the visible set at the start of the next block
    #[state]
    next_visible_slot_number: sov_modules_api::KernelStateValue<TransitionHeight>,

    /// The real slot number of the rollup.
    // This value is also required to create a `KernelWorkingSet`. See note on `visible_height` above.
    #[state]
    true_slot_number: sov_modules_api::KernelStateValue<TransitionHeight>,

    /// The current time, as reported by the DA layer
    #[state]
    time: sov_modules_api::VersionedStateValue<Time>,

    /// A record of all previous state transitions which are available to the VM.
    /// Currently, this includes *all* historical state transitions, but that may change in the future.
    /// This state map is delayed by one transition. In other words - the transition that happens in time i
    /// is stored during transition i+1. This is mainly due to the fact that this structure depends on the
    /// rollup's root hash which is only stored once the transition has completed.
    // TODO: This should be a `VersionedStateMap`, so that recent values are not visible to user-space
    #[state]
    historical_transitions:
        sov_modules_api::StateMap<TransitionHeight, StateTransition<S, Da>, BcsCodec>,

    /// The transition that is currently processed
    #[state]
    in_progress_transition:
        sov_modules_api::VersionedStateValue<TransitionInProgress<S, Da>, BcsCodec>,

    /// The parameters for the state based gas price computation.
    #[state]
    gas_price_state: sov_modules_api::StateValue<GasPriceState<S>>,

    /// The genesis root hash.
    /// Set after the first transaction of the rollup is executed, using the `begin_slot` hook.
    // TODO: This should be made read-only
    #[state]
    genesis_hash: sov_modules_api::StateValue<<S::Storage as Storage>::Root>,
}

impl<S: Spec, Da: DaSpec> ChainState<S, Da> {
    /// Returns transition height in the current slot
    pub fn true_slot_number<T>(&self, working_set: &mut T) -> TransitionHeight
    where
        T: StateReaderAndWriter<Kernel>,
    {
        self.true_slot_number.get(working_set).unwrap_or_default()
    }

    /// Returns transition height for the next slot to start execution
    pub fn next_visible_slot_number<T>(&self, working_set: &mut T) -> TransitionHeight
    where
        T: StateReaderAndWriter<Kernel>,
    {
        self.next_visible_slot_number
            .get(working_set)
            .unwrap_or_default()
    }

    /// Returns transition height in the current slot
    pub fn set_next_visible_slot_number<T>(&self, value: &u64, working_set: &mut T)
    where
        T: StateReaderAndWriter<Kernel>,
    {
        tracing::debug!(slot_number = value, "Setting next visible slot number");
        self.next_visible_slot_number.set(value, working_set);
    }

    /// Returns the current time, as reported by the DA layer
    pub fn get_time(&self, working_set: &mut impl VersionReader) -> Time {
        self.time
            .get_current(working_set)
            .expect("Time must be set at initialization")
    }

    /// Return the genesis hash of the module.
    pub fn get_genesis_hash(
        &self,
        working_set: &mut impl StateAccessor,
    ) -> Option<<S::Storage as Storage>::Root> {
        self.genesis_hash.get(working_set)
    }

    /// Returns the transition in progress of the module.
    pub fn get_in_progress_transition(
        &self,
        working_set: &mut impl VersionReader,
    ) -> Option<TransitionInProgress<S, Da>> {
        self.in_progress_transition.get_current(working_set)
    }

    /// Returns the completed transition associated with the provided `transition_num`.
    pub fn get_historical_transitions(
        &self,
        transition_num: TransitionHeight,
        working_set: &mut impl StateAccessor,
    ) -> Option<StateTransition<S, Da>> {
        self.historical_transitions
            .get(&transition_num, working_set)
    }

    /// Returns the parameters used for the gas price computation.
    pub fn get_gas_price_state(
        &self,
        working_set: &mut impl StateAccessor,
    ) -> Option<GasPriceState<S>> {
        self.gas_price_state.get(working_set)
    }

    /// Replaces the parameters used for the gas price computation.
    pub fn set_gas_price_state(&self, state: &GasPriceState<S>, working_set: &mut WorkingSet<S>) {
        self.gas_price_state.set(state, working_set);
    }
}

impl<S: Spec, Da: DaSpec> KernelModule for ChainState<S, Da> {
    type Spec = S;

    type Config = ChainStateConfig<S>;

    fn genesis_unchecked(
        &self,
        config: &Self::Config,
        working_set: &mut KernelWorkingSet<S>,
    ) -> Result<(), Error> {
        // The initialization logic
        Ok(self.init_module(config, working_set)?)
    }
}
