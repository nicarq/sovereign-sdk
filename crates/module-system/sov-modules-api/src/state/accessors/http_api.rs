use std::collections::HashMap;
use std::sync::Arc;

use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};
use sov_state::{
    namespaces, CompileTimeNamespace, EventContainer, IsValueCached, Namespace,
    ProvableStorageCache, SlotKey, SlotValue, Storage,
};
use tracing::trace;

use super::seal::CachedAccessor;
use super::StateCheckpoint;
use crate::capabilities::{HasKernel, KernelWithSlotMapping};
use crate::gas::GasArray;
use crate::{BasicGasMeter, Gas, GasMeter, GasMeteringError, Spec, TypedEvent, VersionReader};

impl<S: Spec, N: CompileTimeNamespace> CachedAccessor<N> for ApiStateAccessor<S> {
    fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        match N::NAMESPACE {
            Namespace::User => self.user_cache.get_without_caching(
                key,
                &self.storage,
                &self.witness,
                self.storage_version,
            ),
            Namespace::Kernel => self.kernel_cache.get_without_caching(
                key,
                &self.storage,
                &self.witness,
                self.storage_version,
            ),
            Namespace::Accessory => match self.accessory_writes.get(key).cloned() {
                Some(Some(value)) => (Some(value), IsValueCached::Yes),
                Some(None) => (None, IsValueCached::Yes),
                None => (
                    self.storage.get_accessory(key, self.storage_version),
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
    storage_version: Option<u64>,
    visible_slot_number: VisibleSlotNumber,
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
            match self.storage.get_with_proof::<N>(key, self.storage_version) {
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

    fn gas_info(&self) -> crate::GasInfo<S::Gas> {
        self.gas_meter.gas_info()
    }
}

impl<S: Spec> EventContainer for ApiStateAccessor<S> {
    fn add_event<E: 'static + core::marker::Send>(&mut self, event_key: &str, event: E) {
        self.events.push(TypedEvent::new(event_key, event));
    }
}

impl<S: Spec + 'static> ApiStateAccessor<S> {
    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with a gas price of zero at the [`StateCheckpoint::rollup_height_to_access`].
    pub fn new(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
    ) -> Self {
        Self::new_with_price(state_checkpoint, kernel, <S::Gas as Gas>::Price::ZEROED)
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
        visible_slot_number: Option<VisibleSlotNumber>,
    ) -> Self {
        Self::new_with_price_and_height(
            state_checkpoint,
            kernel,
            visible_slot_number,
            <S::Gas as Gas>::Price::ZEROED,
        )
    }

    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with the provided gas price. The rollup height is set to [`StateCheckpoint::rollup_height_to_access`].
    pub fn new_with_price(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        gas_price: <S::Gas as Gas>::Price,
    ) -> Self {
        Self::new_with_price_and_height(state_checkpoint, kernel, None, gas_price)
    }

    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with the provided gas price.
    pub fn new_with_price_and_height(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        visible_slot_number: Option<VisibleSlotNumber>,
        gas_price: <S::Gas as Gas>::Price,
    ) -> Self {
        let delta: &super::internals::Delta<<S as Spec>::Storage> = &state_checkpoint.delta;

        let latest_visible_slot_number = state_checkpoint.rollup_height_to_access().as_visible();

        let mut state = Self {
            storage: delta.inner.clone(),
            witness: Default::default(),
            // TODO: #1490. Remove u64::MAX
            gas_meter: BasicGasMeter::new(u64::MAX, gas_price),
            events: Vec::new(),
            kernel_cache: delta.kernel_cache.clone(),
            user_cache: delta.user_cache.clone(),
            accessory_writes: delta.accessory_writes.clone(),
            kernel: kernel.clone(),
            storage_version: None,
            visible_slot_number: visible_slot_number.unwrap_or(latest_visible_slot_number),
        };

        // A height was specified, so this becomes a historical query rather
        // than accessing the latest state.
        if let Some(visible_slot_number) = visible_slot_number {
            // `storage_version` needs to be set to a true slot number that
            // corresponds to the given visible slot number.
            let true_slot_number =
                kernel.first_true_slot_number_for(visible_slot_number, &mut state);
            trace!(
                %visible_slot_number,
                %latest_visible_slot_number,
                ?true_slot_number,
                "Overriding the storage version"
            );
            state.storage_version = true_slot_number.map(|v| v.get());
        }

        state
    }

    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with the
    /// provided gas price.
    ///
    /// # Warning
    ///
    /// This API is not "officially" supported by [`ApiStateAccessor`], and it
    /// likely should never be exposed over-the-wire e.g. as part of the API.
    #[cfg(feature = "test-utils")]
    pub fn new_with_custom_price_at_true_slot_number(
        state_checkpoint: &StateCheckpoint<S>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        slot_num: SlotNumber,
        gas_price: <S::Gas as Gas>::Price,
    ) -> Self {
        let delta: &super::internals::Delta<<S as Spec>::Storage> = &state_checkpoint.delta;

        Self {
            storage: delta.inner.clone(),
            witness: Default::default(),
            // TODO: #1490. Remove u64::MAX
            gas_meter: BasicGasMeter::new(u64::MAX, gas_price),
            events: Vec::new(),
            kernel_cache: delta.kernel_cache.clone(),
            user_cache: delta.user_cache.clone(),
            accessory_writes: delta.accessory_writes.clone(),
            kernel,
            storage_version: None,
            // This is kinda broken. We're casting the slot number to a
            // visible slot number, which happens to be okay in this case but
            // will lead to bugs if we're not careful.
            visible_slot_number: slot_num.as_visible(),
        }
    }

    /// Creates a new [`ApiStateAccessor`] from the provided Storage with a gas price of zero.
    pub fn from_storage<K: HasKernel<S>>(storage: S::Storage, has_kernel: &K) -> Self {
        let empty_checkpoint = StateCheckpoint::new(storage.clone(), &has_kernel.kernel());
        Self::new(&empty_checkpoint, has_kernel.kernel_with_slot_mapping())
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
            storage_version: self.storage_version,
            visible_slot_number: self.visible_slot_number,
        }
    }

    /// Sets the underlying [`ApiStateAccessor`] to the state at the specified `height`.
    /// The gas price contained in the accessor is set to the base fee per gas at the specified height.
    ///
    /// ## Note
    /// This method has a similar effect to [`ApiStateAccessor::state_at_height`], but it does not clone the underlying [`ApiStateAccessor`].
    /// Events and witness contents are wiped out from the underlying [`ApiStateAccessor`] to ensure consistency with [`ApiStateAccessor::state_at_height`].
    pub fn set_state_to_height(&mut self, height: VisibleSlotNumber) -> anyhow::Result<()> {
        self.visible_slot_number = height;
        self.storage_version = Some(height.get());

        self.events = vec![];
        self.witness = Default::default();

        // Set the state's base fee per gas if there is a relevant value to retrieve from the state.
        let Some(base_fee_per_gas) = self.kernel.clone().base_fee_per_gas_at(height, self) else {
            return Err(anyhow::anyhow!(
                "Impossible to retrieve the base fee per gas for the specified slot."
            ));
        };

        self.gas_meter.set_gas_price(base_fee_per_gas);

        Ok(())
    }

    /// Sets the gas price for the accessor.
    pub fn set_gas_price(&mut self, gas_price: <S::Gas as Gas>::Price) {
        self.gas_meter.set_gas_price(gas_price);
    }

    /// Sets the underlying [`ApiStateAccessor`] to the _visible_ state at the specified `height`.
    /// The gas price contained in the accessor is set to the base fee per gas at the specified height.
    ///
    /// ## Note
    /// This method has a similar effect to [`ApiStateAccessor::visible_state_at_height`], but it does not clone the underlying [`ApiStateAccessor`].
    /// Events and witness contents are wiped out from the underlying [`ApiStateAccessor`] to ensure consistency with [`ApiStateAccessor::visible_state_at_height`].
    pub fn set_state_to_visible_height(&mut self, height: SlotNumber) -> Result<(), anyhow::Error> {
        // We are mapping the provided height to the visible height to have access to the correct visible state.
        let visible_height = self.kernel.clone().visible_rollup_height_at(height, self).ok_or_else(|| anyhow::anyhow!("Impossible to retrieve the visible rollup height associated with the provided input. Please ensure you're querying a valid height"))?;

        self.set_state_to_height(visible_height)?;

        Ok(())
    }

    /// Returns a new accessor which accesses the rollup at the specified `height`.
    /// The gas price contained in the accessor is set to the base fee per gas at the specified height.
    ///
    /// ## Note
    /// This method _clones_ the underlying [`ApiStateAccessor`] without its witness contents or associated events.
    pub fn state_at_height(
        &self,
        height: VisibleSlotNumber,
    ) -> Result<ApiStateAccessor<S>, anyhow::Error> {
        // TODO: Is cloning the caches the correct behavior here?
        let mut state = self.clone_without_witness_or_events();

        state.set_state_to_height(height)?;

        Ok(state)
    }

    /// Returns a new accessor which accesses the _visible_ state of the rollup at the specified height.
    /// The gas price contained in the accessor is set to the base fee per gas at the specified height.
    ///
    /// ## Note
    /// This method _clones_ the underlying [`ApiStateAccessor`] without its witness contents or associated events.
    pub fn visible_state_at_height(
        &self,
        height: SlotNumber,
    ) -> Result<ApiStateAccessor<S>, anyhow::Error> {
        // TODO: Is cloning the caches the correct behavior here?
        let mut state = self.clone_without_witness_or_events();

        state.set_state_to_visible_height(height)?;

        Ok(state)
    }
}

impl<S: Spec> VersionReader for ApiStateAccessor<S> {
    fn rollup_height_to_access(&self) -> SlotNumber {
        self.visible_slot_number.as_true()
    }
}
