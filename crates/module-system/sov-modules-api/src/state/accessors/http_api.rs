use std::collections::HashMap;
use std::sync::Arc;

use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};
use sov_state::{
    namespaces, CompileTimeNamespace, EventContainer, IsValueCached, Namespace,
    ProvableStorageCache, SlotKey, SlotValue, Storage,
};

use super::seal::CachedAccessor;
use super::StateCheckpoint;
use crate::capabilities::{KernelWithSlotMapping, RollupHeight};
use crate::gas::GasArray;
use crate::{BasicGasMeter, Gas, GasMeter, GasMeteringError, Spec, TypedEvent, VersionReader};

impl<S: Spec, N: CompileTimeNamespace> CachedAccessor<N> for ApiStateAccessor<S> {
    fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        match N::NAMESPACE {
            Namespace::User => {
                // TODO: We should cache these values to allow accurate gas cost estimation!
                self.user_cache.get_without_caching(
                    key,
                    &self.storage,
                    &self.witness,
                    self.safe_true_slot_number_to_use,
                )
            }
            Namespace::Kernel => {
                // TODO: We should cache this value *unless visible_slot_number is None*
                // TODO: This is an ugly hack to work around the dual use of self.visible_slot_number.
                // We use it inside `VersionReader` to determine what state the accessor is allowed to access (which requires it to be set)
                // but also here to determine what state version to query *from disk*. Unfortunately, those two numbers don't always agree *during initialization*,
                // so we have to use u64::MAX as a marker value to say "the accessor should be allowed to access any value and should use the latest info from storage as necessary."
                // We should separate out the notion of permissions from the notion of what state version to query from disk.
                let num = if let Some(num) = self.visible_slot_number {
                    if num.get() != u64::MAX {
                        Some(num.as_true())
                    } else {
                        None
                    }
                } else {
                    None
                };
                self.kernel_cache
                    .get_without_caching(key, &self.storage, &self.witness, num)
            }
            Namespace::Accessory => match self.accessory_writes.get(key).cloned() {
                Some(Some(value)) => (Some(value), IsValueCached::Yes),
                Some(None) => (None, IsValueCached::Yes),
                None => (
                    self.storage
                        .get_accessory(key, self.safe_true_slot_number_to_use),
                    IsValueCached::No,
                ),
            },
        }
    }

    fn set_cached(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached {
        match N::NAMESPACE {
            Namespace::User => self.user_cache.set(key, value),
            Namespace::Kernel => self.kernel_cache.set(key, value),
            Namespace::Accessory => {
                if self
                    .accessory_writes
                    .insert(key.clone(), Some(value))
                    .is_none()
                {
                    IsValueCached::No
                } else {
                    IsValueCached::Yes
                }
            }
        }
    }

    fn delete_cached(&mut self, key: &SlotKey) -> IsValueCached {
        match N::NAMESPACE {
            Namespace::User => self.user_cache.delete(key),
            Namespace::Kernel => self.kernel_cache.delete(key),
            Namespace::Accessory => {
                if self.accessory_writes.remove(key).is_none() {
                    IsValueCached::No
                } else {
                    IsValueCached::Yes
                }
            }
        }
    }
}

#[derive(Clone, Debug, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StateToAccess {
    RollupHeight(RollupHeight),
    TrueSlotNumber(SlotNumber),
}

/// A [`crate::StateReaderAndWriter`] designed for use within REST APIs and JSON-RPC.
///
/// It can read and write accessory data as well as "user" and "kernel" data.
#[derive(derive_more::Debug)]
pub struct ApiStateAccessor<S: Spec> {
    #[debug(skip)]
    storage: S::Storage,
    #[debug(skip)]
    witness: <<S as Spec>::Storage as Storage>::Witness,
    events: Vec<TypedEvent>,
    gas_meter: BasicGasMeter<S>,
    kernel_cache: ProvableStorageCache<namespaces::Kernel>,
    user_cache: ProvableStorageCache<namespaces::User>,
    accessory_writes: HashMap<SlotKey, Option<SlotValue>>,
    #[debug(skip)]
    kernel: Arc<dyn KernelWithSlotMapping<S>>,
    // The state requested by the user - either a `RollupHeight` or a `SlotNumber`
    state_to_access: StateToAccess,
    // The visible slot number is always stored in the accessor by the time initialization is complete.
    // Unfortunately, we need to run one query using the accessor in order to determine the correct visible slot number,
    // so we can't make this field non-optional.
    visible_slot_number: Option<VisibleSlotNumber>,
    // The true slot number to use for user/accessory queries. This need not correspond exactly the the accessor's
    // rollup height (if present) but it must be older than the N+1th rollup height.
    //
    // Suppose we have the following rollup heights:
    //  height:           1 -> 2 -> 3 -> 4 -> 5
    //  true_slot_number: 1 -> 4 -> 7 -> 10 -> 13
    //
    // Then a safe true slot number to use for user/accessory queries with rollup height 4 is 7, 8, 9, or 10 since all of these numbers
    // will cause any state *before* 4 to be visible. (Assume that the state checkpoint the accessor is based on contains all the state *of* 4 in memory)
    safe_true_slot_number_to_use: Option<SlotNumber>,
}

#[cfg(feature = "native")]
const _: () = {
    use sov_state::{NativeStorage, ProvableCompileTimeNamespace, StorageProof};

    use crate::{ProvenStateAccessor, StateReaderAndWriter};

    impl<N, S: Spec> ProvenStateAccessor<N> for ApiStateAccessor<S>
    where
        N: ProvableCompileTimeNamespace,
        S::Storage: NativeStorage,
        ApiStateAccessor<S>: StateReaderAndWriter<N>,
    {
        type Proof = <<S as Spec>::Storage as Storage>::Proof;

        fn get_with_proof(&mut self, key: SlotKey) -> Option<StorageProof<Self::Proof>> {
            // In this case, we need to use an exact slot number rather than an approximate one - otherwise
            // the returned merkle proof will be useless.

            // Temporarily give access to all visible slot numbers for the purpose of retrieving the mapping between true and visible slots.
            // We'll set the value back to more scoped permissions at the end of this function
            let visible_slot_num = self.visible_slot_number;
            self.visible_slot_number = Some(VisibleSlotNumber::MAX);
            let slot_num = match self.state_to_access {
                StateToAccess::RollupHeight(rollup_height) => self
                    .kernel
                    .clone()
                    .true_slot_number_at_height(rollup_height, self),
                StateToAccess::TrueSlotNumber(slot_number) => Some(slot_number),
            };
            // We set the permissions back to the original value here
            self.visible_slot_number = visible_slot_num;
            let slot_num = slot_num?;

            match self.storage.get_with_proof::<N>(key, Some(slot_num)) {
                Ok(storage_proof) => Some(storage_proof),
                Err(err) => {
                    tracing::debug!(error = ?err, "Error requesting storage proof");
                    None
                }
            }
        }
    }
};

impl<S: Spec> GasMeter for ApiStateAccessor<S> {
    type Spec = S;

    fn charge_gas(&mut self, gas: &S::Gas) -> Result<(), GasMeteringError<S::Gas>> {
        self.gas_meter.charge_gas(gas)
    }

    fn refund_gas(&mut self, gas: &S::Gas) -> Result<(), GasMeteringError<S::Gas>> {
        self.gas_meter.refund_gas(gas)
    }

    fn charge_linear_gas(
        &mut self,
        amount: &<Self::Spec as Spec>::Gas,
        parameter: u64,
    ) -> anyhow::Result<(), GasMeteringError<<Self::Spec as Spec>::Gas>> {
        self.gas_meter.charge_linear_gas(amount, parameter)
    }

    fn gas_info(&self) -> crate::GasInfo<S::Gas> {
        self.gas_meter.gas_info()
    }
}

impl<S: Spec> EventContainer for ApiStateAccessor<S> {
    fn add_event<E: 'static + core::marker::Send>(&mut self, event_key: &str, event: E) {
        self.events.push(TypedEvent::new(event_key, event));
    }
}

/// An error that can occur when creating an [`ApiStateAccessor`].
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ApiStateAccessorError {
    /// The requested height is not accessible.
    #[error("Impossible to get the rollup state at the specified height. Please ensure you have queried the correct height.")]
    HeightNotAccessible,
}

impl<S: Spec + 'static> ApiStateAccessor<S> {
    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with a gas price of zero at the [`StateCheckpoint::rollup_height_to_access`].
    pub fn new(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
    ) -> Self {
        Self::new_with_height(
            state_checkpoint,
            kernel,
            state_checkpoint.rollup_height_to_access(),
        )
        .expect("Creating an ApiStateCheckpoint without specifying a height is infallible")
    }

    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with a gas price of zero at the [`StateCheckpoint::rollup_height_to_access`].
    pub fn new_with_true_slot_number_dangerous(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        slot_number: SlotNumber,
    ) -> Result<Self, ApiStateAccessorError> {
        Self::new_with_price_and_state_to_access(
            state_checkpoint,
            kernel,
            StateToAccess::TrueSlotNumber(slot_number),
            <S::Gas as Gas>::Price::ZEROED,
        )
    }

    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with a gas
    /// price of zero.
    ///
    /// If the given `visible_slot_number` is `None`, the very latest state
    /// (possibly containing soft-confirmed transactions, if using a preferred
    /// sequencer) will be used. Otherwise, a historical query is performed.
    ///
    /// # Warning
    ///
    /// As of 2024-01-07, **historical** queries for soft-confirmed state that
    /// hasn't been processed by the node yet are not supported.
    pub fn new_with_height(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        rollup_height: RollupHeight,
    ) -> Result<Self, ApiStateAccessorError> {
        Self::new_with_price_and_height(
            state_checkpoint,
            kernel,
            rollup_height,
            <S::Gas as Gas>::Price::ZEROED,
        )
    }

    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with the provided gas price. The rollup height is set to [`StateCheckpoint::rollup_height_to_access`].
    pub fn new_with_price(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        gas_price: <S::Gas as Gas>::Price,
    ) -> Result<Self, ApiStateAccessorError> {
        Self::new_with_price_and_height(
            state_checkpoint,
            kernel,
            state_checkpoint.rollup_height_to_access(),
            gas_price,
        )
    }

    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with the provided gas price.
    pub fn new_with_price_and_height(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        rollup_height: RollupHeight,
        gas_price: <S::Gas as Gas>::Price,
    ) -> Result<Self, ApiStateAccessorError> {
        Self::new_with_price_and_state_to_access(
            state_checkpoint,
            kernel,
            StateToAccess::RollupHeight(rollup_height),
            gas_price,
        )
    }

    /// A specialized constructor for use in sov-test-utils. This constructor matches the unusual semantic of our testing framework, which expects to see
    /// the *next* visible slot number with the *current* rollup height in the accessor as soon as the a slot finishes executing.
    ///
    /// In actual execution, this semantic is not needed because the time between the end of slot N and the start of slot N+1 is neglible. Unfortunately,
    /// our tests are written assuming that all assertions will execute during this miniscule instant of time, so we have to accommodate it for now.
    #[cfg(feature = "test-utils")]
    pub fn new_with_price_and_heights(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        rollup_height: RollupHeight,
        visible_slot_number: VisibleSlotNumber,
        gas_price: <S::Gas as Gas>::Price,
    ) -> Result<Self, ApiStateAccessorError> {
        let mut accessor = Self::new_with_price_and_state_to_access(
            state_checkpoint,
            kernel,
            StateToAccess::RollupHeight(rollup_height),
            gas_price,
        )?;
        accessor.visible_slot_number = Some(visible_slot_number);
        Ok(accessor)
    }

    /// Creates a new [`ApiStateAccessor`] that *queries all state at the provided slot number, regardless of whether that slot number is visible*. This should only be used
    /// when the semantics of the call necessitate it. If you're unsure, use [`ApiStateAccessor::new_with_price_and_height`] and provide a rollup height instead.
    pub fn new_with_price_and_slot_number_dangerous(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        slot_number: SlotNumber,
        gas_price: <S::Gas as Gas>::Price,
    ) -> Result<Self, ApiStateAccessorError> {
        Self::new_with_price_and_state_to_access(
            state_checkpoint,
            kernel,
            StateToAccess::TrueSlotNumber(slot_number),
            gas_price,
        )
    }

    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with the provided gas price.
    fn new_with_price_and_state_to_access(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        state_to_access: StateToAccess,
        gas_price: <S::Gas as Gas>::Price,
    ) -> Result<Self, ApiStateAccessorError> {
        let delta: &super::internals::Delta<<S as Spec>::Storage> = &state_checkpoint.delta;

        let mut out = Self {
            storage: delta.inner.clone(),
            witness: Default::default(),
            // TODO: #1490. Remove u64::MAX
            gas_meter: BasicGasMeter::new_with_gas(<S::Gas as Gas>::max(), gas_price),
            events: Vec::new(),
            kernel_cache: delta.kernel_cache.clone(),
            user_cache: delta.user_cache.clone(),
            accessory_writes: delta.accessory_writes.clone(),
            kernel: kernel.clone(),
            state_to_access,
            visible_slot_number: None,
            safe_true_slot_number_to_use: None,
        };

        out.safe_true_slot_number_to_use = match state_to_access {
            // The current slot may not be stored in the `true_slot_number_at_height` map, so we use the previous slot number.
            StateToAccess::RollupHeight(rollup_height) => {
                // If this is a slot that has already been executed, we know the correct true slot number. Use that.
                kernel.true_slot_number_at_height(rollup_height, &mut out)
            }
            StateToAccess::TrueSlotNumber(slot_number) => Some(slot_number),
        };

        let Some(visible_slot_number) = out.lookup_visible_slot_number() else {
            return Err(ApiStateAccessorError::HeightNotAccessible);
        };
        out.visible_slot_number = Some(visible_slot_number);

        Ok(out)
    }

    fn clone_without_witness_or_events(&self) -> Self {
        Self {
            events: Vec::new(),
            gas_meter: self.gas_meter.clone(),
            storage: self.storage.clone(),
            witness: Default::default(),
            kernel_cache: self.kernel_cache.clone(),
            user_cache: self.user_cache.clone(),
            accessory_writes: self.accessory_writes.clone(),
            kernel: self.kernel.clone(),
            state_to_access: self.state_to_access,
            safe_true_slot_number_to_use: self.safe_true_slot_number_to_use,
            visible_slot_number: self.visible_slot_number,
        }
    }

    fn lookup_visible_slot_number(&mut self) -> Option<VisibleSlotNumber> {
        match self.state_to_access {
            StateToAccess::RollupHeight(rollup_height) => self
                .kernel
                .clone()
                .rollup_height_to_visible_slot_number(rollup_height, self),
            StateToAccess::TrueSlotNumber(slot_number) => {
                let visible_slot_number = VisibleSlotNumber::new_dangerous(slot_number.get());
                Some(visible_slot_number)
            }
        }
    }

    /// Sets the underlying [`ApiStateAccessor`] to the state at the specified `height`.
    /// The gas price contained in the accessor is set to the base fee per gas at the specified height.
    ///
    /// ## Note
    /// This method has a similar effect to [`ApiStateAccessor::state_at_height`], but it does not clone the underlying [`ApiStateAccessor`].
    /// Events and witness contents are wiped out from the underlying [`ApiStateAccessor`] to ensure consistency with [`ApiStateAccessor::state_at_height`].
    pub fn set_state_to_height(&mut self, height: RollupHeight) -> anyhow::Result<()> {
        self.state_to_access = StateToAccess::RollupHeight(height);
        self.events = vec![];
        self.witness = Default::default();
        self.visible_slot_number = None;
        let kernel = self.kernel.clone();
        let safe_true_height_to_use = kernel.true_slot_number_at_height(height, self);
        // Temporarily give access to all visible slots numbers for the purpose of retrieving the base fee per gas.
        self.visible_slot_number = Some(VisibleSlotNumber::MAX);
        // Set the state's base fee per gas if there is a relevant value to retrieve from the state.
        let Some(base_fee_per_gas) = self.kernel.clone().base_fee_per_gas_at(height, self) else {
            return Err(anyhow::anyhow!(
                "Impossible to retrieve the base fee per gas for the specified slot."
            ));
        };
        // Set the visible slot number to the correct value.
        self.visible_slot_number = self.lookup_visible_slot_number();
        self.safe_true_slot_number_to_use = safe_true_height_to_use;

        self.gas_meter.set_gas_price(base_fee_per_gas);

        Ok(())
    }

    /// Sets the gas price for the accessor.
    pub fn set_gas_price(&mut self, gas_price: <S::Gas as Gas>::Price) {
        self.gas_meter.set_gas_price(gas_price);
    }

    /// Returns a new accessor which accesses the rollup at the specified `height`.
    /// The gas price contained in the accessor is set to the base fee per gas at the specified height.
    ///
    /// ## Note
    /// This method _clones_ the underlying [`ApiStateAccessor`] without its witness contents or associated events.
    pub fn state_at_height(
        &self,
        height: RollupHeight,
    ) -> Result<ApiStateAccessor<S>, anyhow::Error> {
        // TODO: Is cloning the caches the correct behavior here?
        let mut state = self.clone_without_witness_or_events();

        state.set_state_to_height(height)?;

        Ok(state)
    }

    /// Get the true slot number being used for queries.
    #[cfg(feature = "test-utils")]
    pub fn true_slot_number_to_use(&self) -> SlotNumber {
        self.safe_true_slot_number_to_use.unwrap()
    }
}

impl<S: Spec> VersionReader for ApiStateAccessor<S> {
    fn visible_slot_number_to_access(&self) -> VisibleSlotNumber {
        self.visible_slot_number
            .expect("Visible slot number must be set during accessor initialization")
    }

    fn rollup_height_to_access(&self) -> RollupHeight {
        match self.state_to_access {
            StateToAccess::RollupHeight(rollup_height) => rollup_height,
            StateToAccess::TrueSlotNumber(_slot_number) => {
                todo!()
            }
        }
    }
}
