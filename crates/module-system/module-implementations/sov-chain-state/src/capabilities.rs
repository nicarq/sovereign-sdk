#[cfg(feature = "native")]
use sov_modules_api::capabilities::KernelWithSlotMapping;
use sov_modules_api::capabilities::RollupHeight;
use sov_modules_api::da::BlockHeaderTrait;
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{DaSpec, GasSpec, KernelStateAccessor, KernelWriter, Spec, StateReader};
use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};
use sov_state::{Kernel, ProvableNamespace, Storage, User};

use crate::{BlockGasInfo, ChainState, SlotInformation, VersionReader};

impl<S: Spec> ChainState<S> {
    /// This is the *pre* execution state root for the specified rollup height, delayed by a configurable number of blocks ("STATE_ROOT_DELAY_BLOCKS").
    ///
    /// We use the *pre* state root because we need to be guaranteed that the root is available in kernel state even if the node
    /// is lagging behind and has not processed another slot.
    ///
    /// ## Note
    /// If the state root at the requested height is not available yet, this method will return `None`.
    pub fn visible_hash_for(
        &self,
        rollup_height: RollupHeight,
        state: &mut KernelStateAccessor<'_, S>,
    ) -> Option<<S::Storage as Storage>::Root> {
        use sov_state::StateRoot;

        let state_root_delay_blocks: u64 = config_value!("STATE_ROOT_DELAY_BLOCKS");
        let rollup_height_to_use: RollupHeight =
            rollup_height.saturating_sub(state_root_delay_blocks);
        let slot_number = self
            .slot_number_history
            .get(&rollup_height_to_use, state)
            .unwrap_infallible()?;

        // We never return anything before the genesis root because the pre-state root from genesis isn't really meaningful.
        if slot_number == VisibleSlotNumber::GENESIS {
            return self
                .pre_state_root_at_height(SlotNumber::ONE, state)
                .unwrap_infallible();
        }

        let kernel_root = self
            .pre_state_root_at_height(slot_number.as_true(), state)
            .unwrap_infallible()?;

        // We have to special case the genesis user space root because we don't call `increment_rollup_height` for the genesis block until
        // we create the first rollup block. Since we use *pre* state roots here, rollup height 1 also uses the genesis state root.
        if rollup_height_to_use <= RollupHeight::ONE {
            Some(kernel_root)
        } else {
            // Return the pre-state root for the rollup_height_to_use
            let user_root = self
                .user_pre_state_root_at_height(rollup_height_to_use, state)
                .unwrap_infallible()?;
            Some(<S::Storage as Storage>::Root::from_namespace_roots(
                user_root,
                kernel_root.namespace_root(ProvableNamespace::Kernel),
            ))
        }
    }

    /// Increments the rollup height stored in state and updates the accessor to match.
    /// ## IMPORTANT
    /// This method assumes that it is called *after* "synchronize_chain" is called.
    pub fn increment_rollup_height(
        &self,
        state: &mut KernelStateAccessor<'_, S>,
        visible_slot_number: VisibleSlotNumber,
        user_state_root: &[u8; 32],
    ) {
        let stale_rollup_height = self.rollup_height(state).unwrap_infallible();
        self.past_user_state_roots
            .set(&stale_rollup_height, user_state_root, state)
            .unwrap_infallible();
        // Update the rollup height
        let next_rollup_height = stale_rollup_height.saturating_add(1);
        self.current_heights
            .set(&(next_rollup_height, visible_slot_number), state)
            .unwrap_infallible();
        self.slot_number_history
            .set(&next_rollup_height, &visible_slot_number, state)
            .unwrap_infallible();
        self.set_next_visible_slot_number(visible_slot_number, state);

        state.update_rollup_height(next_rollup_height);
        state.update_visible_slot_number(visible_slot_number);
    }

    /// Update the chain state at the beginning of the slot. Compute the next gas price
    /// ## IMPORTANT
    /// This method assumes that it is called *before* "increment_rollup_height" is called.
    pub fn synchronize_chain(
        &self,
        slot_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        pre_state_root: &<S::Storage as Storage>::Root,
        state: &mut KernelStateAccessor<S>,
    ) {
        // Start by recording the previous slot's rollup height and visible slot number.
        // Note that the values we fetch here are the ones that were left over from the previous slot, because "increment_rollup_height" is called after synchronize_chain.
        let (leftover_rollup_height, _leftover_visible_slot_number) = self
            .current_heights
            .get(state)
            .unwrap_infallible()
            .expect("Current heights must be set at genesis and updated at each slot");
        self.true_slot_number_history
            .set(
                &leftover_rollup_height,
                &self
                    .true_slot_number
                    .get(state)
                    .unwrap_infallible()
                    .expect("True slot number must be set at genesis and updated at each slot"),
                state,
            )
            .unwrap_infallible();

        // We increment the slot number at the very beginning of the slot execution
        self.increment_true_slot_number(state);

        // There may not be a previous slot if the slot comes right after the genesis block
        // We first extend the slot map because we are going to read from it before we set it.
        let maybe_previous_slot = self
            .slots
            .get(state.visible_slot_number().as_true(), state)
            .unwrap_infallible();

        // We compute the base fee per gas from the previous slot if it exists
        let base_fee_per_gas = maybe_previous_slot
            .map(|previous_slot| Self::compute_base_fee_per_gas(previous_slot.gas_info, 1))
            .unwrap_or_else(|| S::initial_base_fee_per_gas());

        let gas_info = BlockGasInfo::new(
            // TODO(@theochap): the gas limit should be updated dynamically `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/271`
            S::initial_gas_limit(),
            base_fee_per_gas,
        );

        self.slots
            .push(
                &SlotInformation {
                    hash: slot_header.hash(),
                    gas_info,
                    prev_state_root: pre_state_root.clone(),
                },
                state,
            )
            .unwrap_infallible();

        self.time
            .set_true_current(&slot_header.time(), state)
            .unwrap_infallible();
    }

    /// Updates the gas used by the transition in progress at the end of each slot
    pub fn finalize_chain_state(&self, gas_used: &S::Gas, state: &mut KernelStateAccessor<S>) {
        // We retrieve the last slot in progress, update its gas information and store it back to the state
        let mut in_progress_slot = self
            .last_slot(state)
            .unwrap_infallible()
            .expect("There should always be a transition in progress");

        in_progress_slot.gas_info.update_gas_used(gas_used.clone());

        self.slots
            .set_last(&in_progress_slot, state)
            .expect("An error occurred while setting the last slot in progress. This is a bug. Please report it.");

        self.true_to_visible_slot_number_history
            .set_if_absent(
                &state.true_slot_number(),
                // The true slot number was already incremented.
                //
                // TODO: audit this and make sure there's no off-by-one error
                // here.
                &state.visible_slot_number(),
                state,
            )
            .unwrap_infallible();
    }

    /// Returns the gas info from a *previous* rollup height.
    pub fn historical_gas_info_at<
        Reader: VersionReader + StateReader<User, Error = E> + StateReader<Kernel, Error = E>,
        E,
    >(
        &self,
        height: RollupHeight,
        state: &mut Reader,
    ) -> Result<Option<BlockGasInfo<S::Gas>>, <Reader as StateReader<Kernel>>::Error> {
        if height == RollupHeight::GENESIS {
            return Ok(Some(BlockGasInfo::new(
                S::initial_gas_limit(),
                S::initial_base_fee_per_gas(),
            )));
        }

        self.gas_info.get(&height, state)
    }

    /// Returns the base fee per gas accessible at the specified slot height for this state accessor.
    pub fn base_fee_per_gas_at<
        Reader: VersionReader + StateReader<User, Error = E> + StateReader<Kernel, Error = E>,
        E,
    >(
        &self,
        height: RollupHeight,
        state: &mut Reader,
    ) -> Result<
        Option<<S::Gas as sov_modules_api::Gas>::Price>,
        <Reader as StateReader<Kernel>>::Error,
    > {
        if height <= RollupHeight::ONE {
            return Ok(Some(S::initial_base_fee_per_gas()));
        }

        let (current_rollup_height, current_visible_slot_number) = self
            .current_heights
            .get(state)?
            .expect("Current heights must be set at genesis");

        if height == current_rollup_height {
            let prev_gas_info = self
                .historical_gas_info_at(height.saturating_sub(1), state)?
                .expect("Gas info must be set at the end of each slot");
            let prev_visible_height = self
                .visible_slot_number_at_height(height.saturating_sub(1), state)?
                .unwrap_or_else(|| panic!("Slot number history must be set at genesis and updated at each slot. Could not find entry for height: {}", height.saturating_sub(1)));
            let slots_elapsed = current_visible_slot_number
                .get()
                .saturating_sub(prev_visible_height.get());
            let next_base_price = Self::compute_base_fee_per_gas(prev_gas_info, slots_elapsed);
            return Ok(Some(next_base_price));
        }

        Ok(self
            .historical_gas_info_at(height, state)?
            .map(|gas_info| gas_info.base_fee_per_gas().clone()))
    }

    /// Returns the slot gas limit at the specified slot height for this state accessor.
    pub fn block_gas_limit_at<
        Reader: VersionReader + StateReader<User, Error = E> + StateReader<Kernel, Error = E>,
        E,
    >(
        &self,
        height: RollupHeight,
        state: &mut Reader,
    ) -> Result<Option<S::Gas>, <Reader as StateReader<Kernel>>::Error> {
        if height == RollupHeight::GENESIS {
            return Ok(Some(S::initial_gas_limit()));
        }

        let (current_rollup_height, _current_visible_slot_number) = self
            .current_heights
            .get(state)?
            .expect("Current heights must be set at genesis");

        if height == current_rollup_height {
            // TODO(@theochap): the gas limit should be updated dynamically `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/271`
            // TODO: Compute a new gas limit here
            return Ok(Some(S::initial_gas_limit()));
        }

        Ok(self
            .historical_gas_info_at(height, state)?
            .map(|gas_info| gas_info.gas_limit().clone()))
    }

    /// Returns the base fee per gas accessible at the current slot accessible from the version reader.
    /// This value is safe to be used in the transaction execution context.
    ///
    /// ## Note
    /// This method can return `None` if the base fee per gas for the current slot cannot be determined yet.
    /// This can happen when querying a slot too far ahead in the future.
    pub fn base_fee_per_gas<
        Reader: VersionReader + StateReader<User, Error = E> + StateReader<Kernel, Error = E>,
        E,
    >(
        &self,
        state: &mut Reader,
    ) -> Result<
        Option<<S::Gas as sov_modules_api::Gas>::Price>,
        <Reader as StateReader<Kernel>>::Error,
    > {
        self.base_fee_per_gas_at(state.rollup_height_to_access(), state)
    }

    /// Returns the slot gas limit at the current slot accessible from the version reader.
    pub fn block_gas_limit<
        Reader: VersionReader + StateReader<User, Error = E> + StateReader<Kernel, Error = E>,
        E,
    >(
        &self,
        state: &mut Reader,
    ) -> Result<Option<S::Gas>, <Reader as StateReader<Kernel>>::Error> {
        self.block_gas_limit_at(state.rollup_height_to_access(), state)
    }
}

#[cfg(feature = "native")]
const _: () = {
    use sov_modules_api::ApiStateAccessor;
    use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};

    impl<S: Spec> KernelWithSlotMapping<S> for ChainState<S> {
        fn visible_slot_number_at(
            &self,
            true_slot_number: SlotNumber,
            state: &mut ApiStateAccessor<S>,
        ) -> Option<VisibleSlotNumber> {
            self.visible_slot_number_at(true_slot_number, state)
                .unwrap_infallible()
        }

        fn rollup_height_to_visible_slot_number(
            &self,
            height: RollupHeight,
            state: &mut ApiStateAccessor<S>,
        ) -> Option<VisibleSlotNumber> {
            self.slot_number_history
                .get(&height, state)
                .unwrap_infallible()
        }

        fn current_rollup_height(&self, state: &mut ApiStateAccessor<S>) -> RollupHeight {
            self.current_heights
                .get(state)
                .unwrap_infallible()
                .expect("Current heights must be set at genesis")
                .0
        }

        #[cfg(feature = "native")]
        fn true_slot_number_at_historical_height(
            &self,
            height: RollupHeight,
            state: &mut ApiStateAccessor<S>,
        ) -> Option<SlotNumber> {
            if height == RollupHeight::GENESIS {
                return Some(SlotNumber::GENESIS);
            }
            self.true_slot_number_history
                .get(&height, state)
                .unwrap_infallible()
        }

        fn base_fee_per_gas_at(
            &self,
            height: RollupHeight,
            state: &mut ApiStateAccessor<S>,
        ) -> Option<<<S as Spec>::Gas as sov_modules_api::Gas>::Price> {
            self.base_fee_per_gas_at(height, state).unwrap_infallible()
        }
    }
};
