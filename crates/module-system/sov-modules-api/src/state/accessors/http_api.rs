use std::collections::HashMap;
use std::sync::Arc;

use sov_state::{
    namespaces, CompileTimeNamespace, EventContainer, IsValueCached, Namespace,
    ProvableStorageCache, SlotKey, SlotValue, Storage,
};

use super::seal::CachedAccessor;
use super::StateCheckpoint;
use crate::capabilities::{Kernel, KernelWithSlotMapping};
use crate::gas::GasArray;
use crate::{BasicGasMeter, Gas, GasMeter, GasMeteringError, Spec, TypedEvent};

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
pub struct ApiStateAccessor<S: Spec> {
    storage: S::Storage,
    witness: <<S as Spec>::Storage as Storage>::Witness,
    events: Vec<TypedEvent>,
    gas_meter: BasicGasMeter<S::Gas>,
    kernel_cache: ProvableStorageCache<namespaces::Kernel>,
    user_cache: ProvableStorageCache<namespaces::User>,
    accessory_writes: HashMap<SlotKey, Option<SlotValue>>,
    kernel: Arc<dyn KernelWithSlotMapping<S>>,
    storage_version: Option<u64>,
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

impl<S: Spec> GasMeter<S::Gas> for ApiStateAccessor<S> {
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
    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with a gas price of zer.
    pub fn new(
        state_checkpoint: &StateCheckpoint<S::Storage>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        storage_version: Option<u64>,
    ) -> Self {
        Self::new_with_price(
            state_checkpoint,
            kernel,
            storage_version,
            <S::Gas as Gas>::Price::ZEROED,
        )
    }

    /// Creates a new [`ApiStateAccessor`] from a [`StateCheckpoint`] with a gas price of zer.
    pub fn new_with_price(
        state_checkpoint: &StateCheckpoint<S::Storage>,
        kernel: Arc<dyn KernelWithSlotMapping<S>>,
        storage_version: Option<u64>,
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
            storage_version,
        }
    }

    /// Creates a new [`ApiStateAccessor`] from the provided Storage with a gas price of zero.
    pub fn from_storage<K: Kernel<S> + KernelWithSlotMapping<S>>(
        storage: S::Storage,
        kernel: K,
    ) -> Self {
        let empty_checkpoint = StateCheckpoint::new(storage.clone(), &kernel);
        Self::new(&empty_checkpoint, Arc::new(kernel), None)
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
        }
    }

    /// Sets the accessor to return data consistent with the rollup's state
    /// as of the provided slot number.
    pub fn set_rollup_height(&mut self, height: Option<u64>) {
        let visible_height = height.map(|height| {
            let kernel = self.kernel.clone();
            kernel.visible_slot_number_at(height, self)
        });

        self.storage_version = visible_height;
    }

    /// Returns a new accessor which accesses the rollup
    pub fn get_archival_at(&self, height: u64) -> ApiStateAccessor<S> {
        // TODO: Is cloning the gas price the intended behavior here?
        // TODO: Is cloning the caches the correct behavior here?
        let mut state = self.clone_without_witness_or_events();
        state.set_rollup_height(Some(height));
        state
    }
}
