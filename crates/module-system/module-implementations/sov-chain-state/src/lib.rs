#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

use sov_modules_api::capabilities::{BlockGasInfo, RollupHeight};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{KernelStateMap, ModuleRestApi, NotInstantiable, StateCheckpoint, StateMap};
/// Contains the call methods used by the module
mod call;
mod gas;
#[cfg(test)]
mod tests;
use sov_modules_api::{
    BootstrapWorkingSet, CodeCommitmentFor, GenesisState, KernelStateAccessor, ModuleError,
    ModuleId, ModuleInfo, Spec, StateAccessor, StateReader, StateReaderAndWriter,
    VersionedStateVec,
};

mod genesis;

pub use gas::{NonZeroRatio, NonZeroRatioConversionError};
pub use genesis::*;
use sov_modules_api::OperatingMode;

/// Capabilities implementation for the module
pub mod capabilities;

/// The query interface with the module
#[cfg(feature = "native")]
mod query;
use borsh::{BorshDeserialize, BorshSerialize};
use derivative::Derivative;
use serde::{Deserialize, Serialize};
use sov_modules_api::da::Time;
use sov_modules_api::{
    DaSpec, Error, Gas, KernelStateValue, Module, StateValue, ValidityConditionChecker,
    VersionReader, VersionedStateValue,
};
use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};
use sov_state::codec::BcsCodec;
use sov_state::namespaces::Kernel;
use sov_state::{Storage, User};
use tracing::trace;

#[derive(Derivative, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Debug)]
// We need to use derivative here because `Storage` doesn't implement `Eq` and `PartialEq`
#[derivative(PartialEq(bound = "S: Spec"), Eq(bound = "S: Spec"))]
/// Structure that contains the information needed to represent a single state transition.
pub struct StateTransition<S: Spec> {
    post_state_root: <S::Storage as Storage>::Root,
    slot: SlotInformation<S>,
}

impl<S: Spec> StateTransition<S> {
    /// Creates a new state transition.
    pub fn new(
        slot_hash: <<S as Spec>::Da as DaSpec>::SlotHash,
        post_state_root: <S::Storage as Storage>::Root,
        validity_condition: <<S as Spec>::Da as DaSpec>::ValidityCondition,
        gas_info: BlockGasInfo<S::Gas>,
    ) -> Self {
        Self {
            slot: SlotInformation::new(slot_hash, validity_condition, gas_info),
            post_state_root,
        }
    }
}

impl<S: Spec> StateTransition<S> {
    /// Compare the transition block hash and state root with the provided input couple. If
    /// the pairs are equal, return [`true`].
    pub fn compare_hashes(
        &self,
        slot_hash: &<<S as Spec>::Da as DaSpec>::SlotHash,
        post_state_root: &<S::Storage as Storage>::Root,
    ) -> bool {
        self.slot.hash == *slot_hash && self.post_state_root == *post_state_root
    }

    /// Returns the post state root of a state transition
    pub fn post_state_root(&self) -> &<S::Storage as Storage>::Root {
        &self.post_state_root
    }

    /// Returns the slot hash of a state transition
    pub fn slot_hash(&self) -> &<<S as Spec>::Da as DaSpec>::SlotHash {
        &self.slot.hash
    }

    /// Returns the total gas used for the block execution
    pub const fn gas_used(&self) -> &S::Gas {
        self.slot.gas_info.gas_used()
    }

    /// Returns the gas price computed for the block execution
    pub const fn gas_price(&self) -> &<S::Gas as Gas>::Price {
        self.slot.gas_info.base_fee_per_gas()
    }

    /// Returns the gas limit of used for the block execution
    pub const fn gas_limit(&self) -> &S::Gas {
        self.slot.gas_info.gas_limit()
    }

    /// Returns the validity condition associated with the transition
    pub fn validity_condition(&self) -> &<<S as Spec>::Da as DaSpec>::ValidityCondition {
        &self.slot.validity_condition
    }

    /// Checks the validity condition of a state transition
    pub fn validity_condition_check<Checker: ValidityConditionChecker<<<S as Spec>::Da as DaSpec>::ValidityCondition>>(
        &self,
        checker: &mut Checker,
    ) -> Result<(), <Checker as ValidityConditionChecker<<<S as Spec>::Da as DaSpec>::ValidityCondition>>::Error>{
        checker.check(&self.slot.validity_condition)
    }
}

/// Represents a transition in progress for the rollup.
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(bound = "S: Spec")]
pub struct SlotInformation<S: Spec> {
    hash: <<S as Spec>::Da as DaSpec>::SlotHash,
    validity_condition: <<S as Spec>::Da as DaSpec>::ValidityCondition,
    gas_info: BlockGasInfo<S::Gas>,
}

impl<S: Spec> SlotInformation<S> {
    /// Creates a new transition in progress
    pub fn new(
        slot_hash: <<S as Spec>::Da as DaSpec>::SlotHash,
        validity_condition: <<S as Spec>::Da as DaSpec>::ValidityCondition,
        gas_info: BlockGasInfo<S::Gas>,
    ) -> Self {
        Self {
            hash: slot_hash,
            validity_condition,
            gas_info,
        }
    }

    /// Returns the gas price of the transition.
    pub const fn gas_price(&self) -> &<S::Gas as Gas>::Price {
        self.gas_info.base_fee_per_gas()
    }

    /// Returns the total gas used of the transition.
    pub const fn gas_used(&self) -> &S::Gas {
        self.gas_info.gas_used()
    }

    /// Returns the gas limit of the transition.
    pub const fn gas_limit(&self) -> &S::Gas {
        self.gas_info.gas_limit()
    }

    /// Returns the block hash of the transition in progress
    pub const fn hash(&self) -> &<<S as Spec>::Da as DaSpec>::SlotHash {
        &self.hash
    }

    /// Returns the gas info of the transition.
    pub fn gas_info(&self) -> &BlockGasInfo<S::Gas> {
        &self.gas_info
    }
}

/// The chain state module definition. Contains the current state of the da layer.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct ChainState<S: Spec> {
    /// The ID of the module.
    #[id]
    id: ModuleId,

    /// The height that should be loaded as the visible set at the start of the next block
    #[state]
    next_visible_slot_number: KernelStateValue<VisibleSlotNumber>,

    /// The rollup height of the current slot
    // This is a normal state value, since the current rollup height is always known
    #[state]
    current_heights: StateValue<(RollupHeight, VisibleSlotNumber)>,

    #[state]
    slot_number_history: StateMap<RollupHeight, VisibleSlotNumber>,

    #[state]
    true_slot_number_history: KernelStateMap<RollupHeight, SlotNumber>,

    #[state]
    true_to_visible_slot_number_history:
        sov_modules_api::KernelStateMap<SlotNumber, VisibleSlotNumber>,

    /// The real rollup height of the rollup.
    /// This value is also required to create a [`sov_state::storage::KernelStateAccessor`]. See note on `visible_slot_number` above.
    #[state]
    true_slot_number: KernelStateValue<SlotNumber>,

    /// The current time, as reported by the DA layer
    #[state]
    time: VersionedStateValue<Time>,

    /// The mode that the rollup is operating in.
    #[state]
    operating_mode: StateValue<OperatingMode>,

    /// A record of all previous slots' information which are available to the VM.
    /// Currently, this includes *all* slots, but that may change in the future.
    #[state]
    slots: VersionedStateVec<SlotInformation<S>, BcsCodec>,

    /// A record of all previous rollup heights' gas information.
    #[state]
    gas_info: StateMap<RollupHeight, BlockGasInfo<S::Gas>>,

    /// The state root hashes from genesis to the current slot.
    /// ## Note
    /// There is a one slot-delay for the update of this state map because we cannot predict what will be the next
    /// most up to date state root inside the current slot. We have to wait for the next slot to start getting processed and return
    /// the pre-state root.
    #[state]
    state_roots: VersionedStateVec<<S::Storage as Storage>::Root, BcsCodec>,

    /// The height of the first DA block.
    /// Set at the rollup genesis. Since the rollup is always delayed by a constant amount of blocks,
    /// we can use this value with the `true_slot_number` to get the current height of the DA layer,
    /// using the following formula:
    /// `current_da_height = true_slot_number + genesis_da_height`.
    /// Should be the same as the `genesis_height` field in the `RunnerConfig` (`sov-stf-runner` crate)
    #[state]
    genesis_da_height: StateValue<u64>,

    /// The rollup's code commitment.
    /// This value is initialized at genesis and can be used to verify the rollup's execution.
    /// This value is used by the `AttesterIncentives` module to verify challenges of attestations.
    #[state]
    inner_code_commitment: StateValue<CodeCommitmentFor<S::InnerZkvm>, BcsCodec>,

    /// Aggregated code commitment.
    /// This value is initialized at genesis and can be used in the aggregated proving circuit to
    /// verify the rollup execution from genesis to the current slot.
    /// This value is used by the `ProverIncentives` module to verify the proofs posted on the DA layer.
    #[state]
    outer_code_commitment: StateValue<CodeCommitmentFor<S::OuterZkvm>, BcsCodec>,
}

impl<S: Spec> ChainState<S> {
    /// Returns transition height in the current slot
    pub fn true_slot_number<T>(
        &self,
        state: &mut T,
    ) -> Result<SlotNumber, <T as StateReader<Kernel>>::Error>
    where
        T: StateReaderAndWriter<Kernel>,
    {
        Ok(self.true_slot_number.get(state)?.unwrap_or_default())
    }

    /// Returns slot number for the next slot to start execution
    pub fn next_visible_slot_number(
        &self,
        state: &mut BootstrapWorkingSet<'_, S::Storage>,
    ) -> VisibleSlotNumber {
        self.next_visible_slot_number
            .get(state)
            .unwrap_infallible()
            .unwrap_or_default()
    }

    /// Returns the visible rollup height corresponding to the provided real slot.
    pub fn visible_slot_number_at<T>(
        &self,
        true_slot_number: SlotNumber,
        state: &mut T,
    ) -> Result<Option<VisibleSlotNumber>, T::Error>
    where
        T: StateReader<Kernel>,
    {
        if true_slot_number == SlotNumber::GENESIS {
            return Ok(Some(VisibleSlotNumber::GENESIS));
        }

        let visible_slot_number = self
            .true_to_visible_slot_number_history
            .get(&true_slot_number, state)?;

        trace!(?visible_slot_number, %true_slot_number, "ChainState::visible_slot_number_at");

        Ok(visible_slot_number)
    }

    /// Returns transition height in the current slot
    pub fn set_next_visible_slot_number(
        &self,
        next_visible_slot_number: VisibleSlotNumber,
        state: &mut KernelStateAccessor<S>,
    ) {
        tracing::debug!(%next_visible_slot_number, "Setting next visible slot number");

        self.next_visible_slot_number
            .set(&next_visible_slot_number, state)
            .unwrap_infallible();
    }

    /// Returns the current time, as reported by the DA layer. This can be called within the execution context of a transaction.
    pub fn get_time<Reader: VersionReader>(
        &self,
        state: &mut Reader,
    ) -> Result<Time, <Reader as StateReader<Kernel>>::Error> {
        Ok(self
            .time
            .get_current(state)?
            .expect("Time must be set at initialization"))
    }

    /// Return the genesis hash of the module.
    pub fn get_genesis_hash<Accessor: VersionReader>(
        &self,
        state: &mut Accessor,
    ) -> Result<Option<<S::Storage as Storage>::Root>, Accessor::Error> {
        self.state_roots.get(SlotNumber::GENESIS, state)
    }

    /// Return the code commitment to be used for verifying the rollup's execution
    /// for each state transition.
    pub fn inner_code_commitment<Accessor: StateAccessor>(
        &self,
        state: &mut Accessor,
    ) -> Result<Option<CodeCommitmentFor<S::InnerZkvm>>, <Accessor as StateReader<User>>::Error>
    {
        self.inner_code_commitment.get(state)
    }

    /// Return the code commitment to be used for verifying the rollup's execution from genesis to the current slot
    /// in the aggregated proving circuit.
    pub fn outer_code_commitment<Accessor: StateAccessor>(
        &self,
        state: &mut Accessor,
    ) -> Result<Option<CodeCommitmentFor<S::OuterZkvm>>, <Accessor as StateReader<User>>::Error>
    {
        self.outer_code_commitment.get(state)
    }

    /// Return the initial height of the DA layer.
    pub fn genesis_da_height<Accessor: StateAccessor>(
        &self,
        state: &mut Accessor,
    ) -> Result<Option<u64>, <Accessor as StateReader<User>>::Error> {
        self.genesis_da_height.get(state)
    }

    /// Returns the last slot processed by the module.
    pub fn last_slot<Reader: VersionReader>(
        &self,
        state: &mut Reader,
    ) -> Result<Option<SlotInformation<S>>, Reader::Error> {
        self.slots.last(state)
    }

    /// Returns the last root processed by the module.
    pub fn last_root<Reader: VersionReader>(
        &self,
        state: &mut Reader,
    ) -> Result<Option<<S::Storage as Storage>::Root>, Reader::Error> {
        self.state_roots.last(state)
    }

    /// Returns the root hash of the state at the provided height.
    pub fn root_at_height<Accessor: VersionReader>(
        &self,
        slot_number: SlotNumber,
        state: &mut Accessor,
    ) -> Result<Option<<S::Storage as Storage>::Root>, Accessor::Error> {
        self.state_roots.get(slot_number, state)
    }

    /// Returns the slot information from the state at the provided height.
    pub fn slot_at_height<Accessor: VersionReader>(
        &self,
        slot_number: SlotNumber,
        state: &mut Accessor,
    ) -> Result<Option<SlotInformation<S>>, Accessor::Error> {
        self.slots.get(slot_number, state)
    }

    /// Returns the completed transition associated with the provided `transition_num`.
    pub fn get_historical_transitions<Accessor: VersionReader>(
        &self,
        slot_number: SlotNumber,
        state: &mut Accessor,
    ) -> Result<Option<StateTransition<S>>, Accessor::Error> {
        if let Some(root) = self.state_roots.get(slot_number, state)? {
            return Ok({
                let maybe_slot = self.slots.get(slot_number, state)?;

                maybe_slot.map(|slot| StateTransition {
                    post_state_root: root,
                    slot,
                })
            });
        }

        Ok(None)
    }

    /// Record the gas usage for a given rollup height.
    pub fn record_gas_usage(
        &self,
        state: &mut StateCheckpoint<S>,
        final_gas_info: BlockGasInfo<S::Gas>,
        rollup_height: RollupHeight,
    ) {
        self.gas_info
            .set(&rollup_height, &final_gas_info, state)
            .unwrap_infallible();
    }

    /// Returns the current operating mode of the rollup.
    pub fn operating_mode<Accessor: StateReader<User>>(
        &self,
        state: &mut Accessor,
    ) -> Result<OperatingMode, Accessor::Error> {
        Ok(self
            .operating_mode
            .get(state)?
            .expect("Operating mode must be set at initialization"))
    }

    /// Get the current rollup height
    pub fn rollup_height<Accessor: StateReader<User>>(
        &self,
        state: &mut Accessor,
    ) -> Result<RollupHeight, Accessor::Error> {
        Ok(self
            .current_heights
            .get(state)?
            .map(|(rollup_height, _)| rollup_height)
            .unwrap_or(RollupHeight::GENESIS))
    }

    /// Returns the visible slot number at the provided height, if that height exists.
    pub fn visible_slot_number_at_height<Accessor: StateReader<User>>(
        &self,
        height: RollupHeight,
        state: &mut Accessor,
    ) -> Result<Option<VisibleSlotNumber>, Accessor::Error> {
        self.slot_number_history.get(&height, state)
    }
}

impl<S: Spec> Module for ChainState<S> {
    type Spec = S;

    type CallMessage = NotInstantiable;

    type Config = ChainStateConfig<S>;

    type Event = ();

    /// Genesis is called when a rollup is deployed and can be used to set initial state values in the module.
    fn genesis(
        &self,
        genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        validity_condition: &<<S as Spec>::Da as DaSpec>::ValidityCondition,
        config: &Self::Config,
        state: &mut impl GenesisState<Self::Spec>,
    ) -> Result<(), ModuleError> {
        // The initialization logic
        Ok(self.init_module(genesis_rollup_header, validity_condition, config, state)?)
    }

    fn call(
        &self,
        _message: Self::CallMessage,
        _context: &sov_modules_api::Context<Self::Spec>,
        _state: &mut impl sov_modules_api::TxState<Self::Spec>,
    ) -> Result<(), Error> {
        Ok(())
    }
}
