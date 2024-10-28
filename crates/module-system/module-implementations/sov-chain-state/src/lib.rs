#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::NotInstantiable;
/// Contains the call methods used by the module
mod call;
mod gas;
#[cfg(test)]
mod tests;
use sov_modules_api::{
    BootstrapWorkingSet, GenesisState, KernelStateAccessor, ModuleError, ModuleId, ModuleInfo,
    Spec, StateAccessor, StateReader, StateReaderAndWriter, VersionedStateVec, Zkvm,
};

mod genesis;

pub use gas::{NonZeroRatio, NonZeroRatioConversionError};
pub use genesis::*;
use serde::de::DeserializeOwned;
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
pub use sov_modules_api::hooks::TransitionHeight;
use sov_modules_api::{
    DaSpec, Error, Gas, KernelStateValue, Module, StateValue, ValidityConditionChecker,
    VersionReader, VersionedStateValue,
};
use sov_state::codec::BcsCodec;
use sov_state::namespaces::Kernel;
use sov_state::{Storage, User};

/// Type alias that contains the height of a given transition
pub type VirtualSlotNumber = u64;

/// A structure that contains block gas information.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, BorshSerialize, BorshDeserialize)]
#[serde(bound = "GU: DeserializeOwned")]
pub struct BlockGasInfo<GU: Gas> {
    /// The gas limit of the block execution.
    /// This value is dynamically adjusted over time to account for the increase
    /// in proving/execution performance.
    gas_limit: GU,
    /// The gas used by the block execution.
    /// This value is set to zero at the beginning of the block execution (in the [`ChainState::synchronize_chain`] capability),
    /// and is populated once the block execution is complete.
    gas_used: GU,
    /// The base fee per gas used for the block execution. This value combined with the `gas_used`
    /// can be used to compute the total base fee (expressed in gas tokens) paid by the block execution.
    base_fee_per_gas: GU::Price,
}

impl<GU: Gas> BlockGasInfo<GU> {
    /// Creates a new [`BlockGasInfo`] with the provided gas limit and base fee per gas.
    /// The `gas_used` is set to zero. This method is meant to be called from the [`ChainState::synchronize_chain`] capability.
    pub fn new(gas_limit: GU, base_fee_per_gas: GU::Price) -> Self {
        Self {
            gas_limit,
            gas_used: GU::zero(),
            base_fee_per_gas,
        }
    }

    /// Updates the gas used by the block execution.
    /// This method is meant to be called from the [`ChainState::finalize_chain_state`] capability.
    pub fn update_gas_used(&mut self, gas_used: GU) {
        self.gas_used = gas_used;
    }

    /// Returns the gas limit of the block execution.
    pub fn gas_limit(&self) -> &GU {
        &self.gas_limit
    }

    /// Returns the gas used by the block execution.
    pub fn gas_used(&self) -> &GU {
        &self.gas_used
    }

    /// Returns the base fee per gas used for the block execution.
    pub fn base_fee_per_gas(&self) -> &GU::Price {
        &self.base_fee_per_gas
    }
}

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
        &self.slot.gas_info.gas_used
    }

    /// Returns the gas price computed for the block execution
    pub const fn gas_price(&self) -> &<S::Gas as Gas>::Price {
        &self.slot.gas_info.base_fee_per_gas
    }

    /// Returns the gas limit of used for the block execution
    pub const fn gas_limit(&self) -> &S::Gas {
        &self.slot.gas_info.gas_limit
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
        &self.gas_info.base_fee_per_gas
    }

    /// Returns the total gas used of the transition.
    pub const fn gas_used(&self) -> &S::Gas {
        &self.gas_info.gas_used
    }

    /// Returns the gas limit of the transition.
    pub const fn gas_limit(&self) -> &S::Gas {
        &self.gas_info.gas_limit
    }

    /// Returns the block hash of the transition in progress
    pub const fn hash(&self) -> &<<S as Spec>::Da as DaSpec>::SlotHash {
        &self.hash
    }
}

/// The chain state module definition. Contains the current state of the da layer.
#[derive(Clone, ModuleInfo)]
pub struct ChainState<S: Spec> {
    /// The ID of the module.
    #[id]
    id: ModuleId,

    /// The height that should be loaded as the visible set at the start of the next block
    #[state]
    next_visible_slot_number: KernelStateValue<TransitionHeight>,

    #[state]
    true_to_virtual_slot_number_history:
        sov_modules_api::KernelStateMap<TransitionHeight, TransitionHeight>,

    /// The real slot number of the rollup.
    /// This value is also required to create a [`sov_state::storage::KernelStateAccessor`]. See note on `visible_height` above.
    #[state]
    true_slot_number: KernelStateValue<TransitionHeight>,

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
    genesis_da_height: StateValue<TransitionHeight>,

    /// The rollup's code commitment.
    /// This value is initialized at genesis and can be used to verify the rollup's execution.
    /// This value is used by the `AttesterIncentives` module to verify challenges of attestations.
    #[state]
    inner_code_commitment: StateValue<<S::InnerZkvm as Zkvm>::CodeCommitment, BcsCodec>,

    /// Aggregated code commitment.
    /// This value is initialized at genesis and can be used in the aggregated proving circuit to
    /// verify the rollup execution from genesis to the current slot.
    /// This value is used by the `ProverIncentives` module to verify the proofs posted on the DA layer.
    #[state]
    outer_code_commitment: StateValue<<S::OuterZkvm as Zkvm>::CodeCommitment, BcsCodec>,
}

impl<S: Spec> ChainState<S> {
    /// Returns transition height in the current slot
    pub fn true_slot_number<T>(
        &self,
        state: &mut T,
    ) -> Result<TransitionHeight, <T as StateReader<Kernel>>::Error>
    where
        T: StateReaderAndWriter<Kernel>,
    {
        Ok(self.true_slot_number.get(state)?.unwrap_or_default())
    }

    /// Returns transition height for the next slot to start execution
    pub fn next_visible_slot_number(
        &self,
        state: &mut BootstrapWorkingSet<'_, S::Storage>,
    ) -> TransitionHeight {
        self.next_visible_slot_number
            .get(state)
            .unwrap_infallible()
            .unwrap_or_default()
    }

    /// Returns the visible slot number corresponding to the provided real slot.
    pub fn visible_slot_number_at<T>(
        &self,
        true_slot_number: u64,
        state: &mut T,
    ) -> Result<TransitionHeight, T::Error>
    where
        T: StateReader<Kernel>,
    {
        let visible_slot_number = self
            .true_to_virtual_slot_number_history
            .get(&true_slot_number, state)?
            .unwrap_or_default();

        dbg!(true_slot_number, visible_slot_number);
        Ok(visible_slot_number)
    }

    /// Returns transition height in the current slot
    pub fn set_next_visible_slot_number(
        &self,
        value: &u64,
        state: &mut KernelStateAccessor<S::Storage>,
    ) {
        tracing::debug!(slot_number = value, "Setting next visible slot number");

        self.next_visible_slot_number
            .set(value, state)
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
        self.state_roots.get(0, state)
    }

    /// Return the code commitment to be used for verifying the rollup's execution
    /// for each state transition.
    pub fn inner_code_commitment<Accessor: StateAccessor>(
        &self,
        state: &mut Accessor,
    ) -> Result<
        Option<<S::InnerZkvm as Zkvm>::CodeCommitment>,
        <Accessor as StateReader<User>>::Error,
    > {
        self.inner_code_commitment.get(state)
    }

    /// Return the code commitment to be used for verifying the rollup's execution from genesis to the current slot
    /// in the aggregated proving circuit.
    pub fn outer_code_commitment<Accessor: StateAccessor>(
        &self,
        state: &mut Accessor,
    ) -> Result<
        Option<<S::OuterZkvm as Zkvm>::CodeCommitment>,
        <Accessor as StateReader<User>>::Error,
    > {
        self.outer_code_commitment.get(state)
    }

    /// Return the initial height of the DA layer.
    pub fn genesis_da_height<Accessor: StateAccessor>(
        &self,
        state: &mut Accessor,
    ) -> Result<Option<TransitionHeight>, <Accessor as StateReader<User>>::Error> {
        self.genesis_da_height.get(state)
    }

    /// Returns the last slot processed by the module.
    pub fn get_last_slot<Reader: VersionReader>(
        &self,
        state: &mut Reader,
    ) -> Result<Option<SlotInformation<S>>, Reader::Error> {
        self.slots.get(state.rollup_height_to_access(), state)
    }

    /// Returns the root hash of the state at the provided height.
    pub fn get_root_at_height<Accessor: VersionReader>(
        &self,
        transition_num: TransitionHeight,
        state: &mut Accessor,
    ) -> Result<Option<<S::Storage as Storage>::Root>, Accessor::Error> {
        self.state_roots.get(transition_num, state)
    }

    /// Returns the completed transition associated with the provided `transition_num`.
    pub fn get_historical_transitions<Accessor: VersionReader>(
        &self,
        transition_num: TransitionHeight,
        state: &mut Accessor,
    ) -> Result<Option<StateTransition<S>>, Accessor::Error> {
        if let Some(root) = self.state_roots.get(transition_num, state)? {
            return Ok({
                let maybe_slot = self.slots.get(transition_num, state)?;

                maybe_slot.map(|slot| StateTransition {
                    post_state_root: root,
                    slot,
                })
            });
        }

        Ok(None)
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
    ) -> Result<sov_modules_api::CallResponse, Error> {
        Ok(Default::default())
    }
}
