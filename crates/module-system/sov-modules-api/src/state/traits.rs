use std::convert::Infallible;
use std::fmt::Debug;

use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};
#[cfg(feature = "native")]
use sov_state::StorageProof;
use sov_state::{
    namespaces, Accessory, CompileTimeNamespace, EventContainer, IsValueCached, Kernel,
    ProvableCompileTimeNamespace, ProvableNamespace, SlotKey, SlotValue, StateCodec,
    StateItemCodec, StateItemDecoder, User,
};
use thiserror::Error;

use super::accessors::seal::UniversalStateAccessor;
use crate::capabilities::RollupHeight;
#[cfg(any(feature = "test-utils", feature = "evm"))]
use crate::UnmeteredStateWrapper;
use crate::{Gas, GasArray, GasMeter, GasMeteringError, GasSpec, Spec};

/// A type that can both read and write the normal "user-space" state of the rollup.
///
/// ```
/// fn delete_state_string<Accessor: sov_modules_api::StateAccessor>(value: sov_modules_api::StateValue<String>, state: &mut Accessor)
///  -> Result<(), <Accessor as sov_modules_api::StateWriter<sov_state::User>>::Error> {
///     if let Some(original) = value.get(state)? {
///         println!("original: {}", original);
///     }
///     value.delete(state)?;
///     Ok(())
/// }
///
///
/// ```
pub trait StateAccessor: StateReaderAndWriter<User> {
    /// Converts this accessor into an [`UnmeteredStateWrapper`]. This method should only be used either in tests or in the `EVM` module.
    #[cfg(any(feature = "test-utils", feature = "evm"))]
    fn to_unmetered(&mut self) -> UnmeteredStateWrapper<Self>
    where
        Self: Sized,
    {
        UnmeteredStateWrapper { inner: self }
    }
}

/// A trait that represents a [`StateAccessor`] that never fails on state accesses. Accessing the state with structs that implement
/// this trait will return [`Infallible`].
///
/// ## Usage example
/// ```
/// use sov_modules_api::prelude::UnwrapInfallible;
///
/// fn delete_state_string<InfallibleAccessor: sov_modules_api::InfallibleStateAccessor>(value: sov_modules_api::StateValue<String>, state: &mut InfallibleAccessor)
///  -> () {
///     if let Some(original) = value.get(state).unwrap_infallible() {
///         println!("original: {}", original);
///     }
///     value.delete(state).unwrap_infallible();
/// }
/// ```
pub trait InfallibleStateAccessor:
    StateReader<User, Error = Infallible> + StateWriter<User, Error = Infallible>
{
}

impl<T> StateAccessor for T where T: StateReaderAndWriter<User> {}

impl<T> InfallibleStateAccessor for T where
    T: StateReader<User, Error = Infallible> + StateWriter<User, Error = Infallible>
{
}

/// Like [`InfallibleStateAccessor`], but for the [`Kernel`] access.
pub trait InfallibleKernelStateAccessor:
    StateReader<Kernel, Error = Infallible> + StateWriter<Kernel, Error = Infallible>
{
}

impl<T> InfallibleKernelStateAccessor for T where
    T: StateReader<Kernel, Error = Infallible> + StateWriter<Kernel, Error = Infallible>
{
}

/// The state accessor used during transaction execution. It provides unrestricted
/// access to [`User`]-space state, as well as limited visibility into the `Kernel` state.
pub trait TxState<S: Spec>:
    StateReader<User, Error: Into<anyhow::Error>>
    + StateReader<Kernel, Error = <Self as StateReader<User>>::Error>
    + StateWriter<User, Error = <Self as StateReader<User>>::Error>
    + StateWriter<Accessory, Error = Infallible>
    + VersionReader
    + EventContainer
    + GasMeter<Spec = S>
{
}

impl<S: Spec, T> TxState<S> for T where
    T: StateReader<User, Error: Into<anyhow::Error>>
        + StateReader<Kernel, Error = <Self as StateReader<User>>::Error>
        + StateWriter<User, Error = <Self as StateReader<User>>::Error>
        + StateWriter<Accessory, Error = Infallible>
        + VersionReader
        + EventContainer
        + GasMeter<Spec = S>
{
}

/// The state accessor used during genesis. It provides unrestricted
/// access to [`User`] and `Kernel` state, as well as limited visibility into [`Accessory`] state.  
pub trait GenesisState<S: Spec>:
    TxState<S> + KernelWriter<Error = <Self as StateReader<User>>::Error>
{
}

/// The set of errors that can be raised during state accesses. For now all these errors are
/// caused by gas metering issues, hence this error type is a wrapper around the [`GasMeteringError`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StateAccessorError<GU: Gas> {
    /// An error occurred when trying to get a value from the state.
    #[error(
        "An error occured while trying to get the value (key {key:?}) from the state: {inner}, namespace: {namespace:?}"
    )]
    Get {
        /// The key of the value that was not found.
        key: SlotKey,
        /// The error that occurred while trying to get the value.
        inner: GasMeteringError<GU>,
        /// The namespace that was queried.
        namespace: ProvableNamespace,
    },
    /// An error occurred when trying to set a value in the state.
    #[error(
        "An error occured while trying to set the value (key {key:?}) in the state: {inner}, namespace: {namespace:?}"
    )]
    Set {
        /// The key of the value that was not found.
        key: SlotKey,
        /// The error that occurred while trying to set the value.
        inner: GasMeteringError<GU>,
        /// The namespace that was queried.
        namespace: ProvableNamespace,
    },
    /// An error occurred when trying to decode a value retrieved from the state.
    #[error(
        "An error occured while trying to decode the value (key {key:?}) in the state: {inner}, namespace: {namespace:?}"
    )]
    Decode {
        /// The key of the value that was not found.
        key: SlotKey,
        /// The error that occurred while trying to decode the value.
        inner: GasMeteringError<GU>,
        /// The namespace that was queried.
        namespace: ProvableNamespace,
    },
    /// An error occurred when trying to delete a value from the state.
    #[error(
        "An error occured while trying to delete the value (key {key:?}) in the state: {inner}, namespace: {namespace:?}"
    )]
    Delete {
        /// The key of the value that was not found.
        key: SlotKey,
        /// The error that occurred while trying to delete the value.
        inner: GasMeteringError<GU>,
        /// The namespace that was queried.
        namespace: ProvableNamespace,
    },
}

/// Returns the gas to charge for a decoding operation.
///
/// ## NOTE
/// The constants' value should be updated based on benchmarks to ensure that the gas cost of the read operation is
/// optimal
pub(crate) fn charge_decode_gas_cost<S: Spec>(
    input: &SlotValue,
    meter: &mut impl GasMeter<Spec = S>,
) -> Result<(), GasMeteringError<S::Gas>> {
    let gas_cost = S::gas_to_charge_for_decoding();
    let input_len = input.value().len();
    meter.charge_linear_gas(&gas_cost, input_len as u64)?;

    Ok(())
}

/// A trait that represents a [`StateReader`] and [`StateWriter`] to a given namespace that never fails on state accesses. Accessing the state with structs that implement
/// this trait will return [`Infallible`].
///
/// ## Usage example
/// ```
/// use sov_modules_api::prelude::UnwrapInfallible;
/// use sov_state::namespaces::User;
///
/// fn delete_state_string<InfallibleAccessor: sov_modules_api::InfallibleStateReaderAndWriter<User>>
/// (value: sov_modules_api::StateValue<String>, state: &mut InfallibleAccessor) -> () {
///     if let Some(original) = value.get(state).unwrap_infallible() {
///         println!("original: {}", original);
///     }
///     value.delete(state).unwrap_infallible();
/// }
/// ```
pub trait InfallibleStateReaderAndWriter<N: CompileTimeNamespace>:
    StateReader<N, Error = Infallible> + StateWriter<N, Error = Infallible>
{
}

impl<
        T: StateReader<N, Error = Infallible> + StateWriter<N, Error = Infallible>,
        N: CompileTimeNamespace,
    > InfallibleStateReaderAndWriter<N> for T
{
}

/// A trait that represents a [`StateReader`] and [`StateWriter`] to the accessory namespace that never fails on state accesses.
/// Basically a [`InfallibleStateReaderAndWriter<Accessory>`] for the accessory namespace.
pub trait AccessoryStateReaderAndWriter: InfallibleStateReaderAndWriter<Accessory> {}
impl<T: InfallibleStateReaderAndWriter<Accessory>> AccessoryStateReaderAndWriter for T {}

/// A wrapper trait for storage reader and writer that can be used to charge gas
/// for the read/write operations.
pub trait StateReaderAndWriter<N: CompileTimeNamespace>:
    StateReader<N> + StateWriter<N, Error = <Self as StateReader<N>>::Error>
{
    /// Removes a value from storage and decode the result
    fn remove_decoded<V, Codec>(
        &mut self,
        key: &SlotKey,
        codec: &Codec,
    ) -> Result<Option<V>, <Self as StateWriter<N>>::Error>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
    {
        let value = self.get_decoded(key, codec)?;
        <Self as StateWriter<N>>::delete(self, key)?;
        Ok(value)
    }
}

impl<T, N> StateReaderAndWriter<N> for T
where
    T: StateReader<N> + StateWriter<N, Error = <Self as StateReader<N>>::Error>,
    N: CompileTimeNamespace,
{
}

/// A storage reader which can access a particular namespace.
pub trait StateReader<N: CompileTimeNamespace>: UniversalStateAccessor {
    /// The error type returned when a state read operation fails.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Get a value from the storage. Basically a wrapper around [`StateReader::get`].
    ///
    /// ## Error
    /// This method can fail if the gas meter doesn't have enough funds to pay for the read operation.
    fn get(&mut self, key: &SlotKey) -> Result<Option<SlotValue>, Self::Error>;

    /// Get a decoded value from the storage.
    ///
    /// ## Error
    /// This method can fail if the gas meter doesn't have enough funds to pay for the read operation.
    ///
    /// ## Note
    /// For now this method doesn't charge an extra amount of gas for the decoding operation. This may change in the future.
    fn get_decoded<V, Codec>(
        &mut self,
        storage_key: &SlotKey,
        codec: &Codec,
    ) -> Result<Option<V>, Self::Error>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>;
}

/// A storage reader which can access the accessory state.
/// Does not charge gas for read operations.
pub trait AccessoryStateReader: UniversalStateAccessor {}

/// A trait wrapper that replicates the functionality of [`StateReader`] but with a gas metering interface.
/// This allows a storage reader to charge gas for read operations.
pub trait ProvableStateReader<N: ProvableCompileTimeNamespace>:
    UniversalStateAccessor + GasMeter
{
}

macro_rules! blanket_impl_metered_state_reader {
    ($namespace:ty) => {
        type Error = StateAccessorError<<T::Spec as GasSpec>::Gas>;

        fn get(&mut self, key: &SlotKey) -> Result<Option<SlotValue>, Self::Error> {
            self.charge_gas(&<T::Spec as GasSpec>::gas_to_charge_for_access())
                .map_err(|e| StateAccessorError::Get{
                    key: key.clone(),
                    inner: e,
                    namespace: <$namespace>::PROVABLE_NAMESPACE,
                })?;


            let (val, is_value_cached) = <Self as  super::accessors::seal::UniversalStateAccessor>::get_value(
                self,
                <$namespace as sov_state::CompileTimeNamespace>::NAMESPACE,
                key,
            );

            if is_value_cached == IsValueCached::Yes {
                self.refund_gas(&<T::Spec as GasSpec>::gas_to_refund_for_hot_access()).expect("Failed to refund gas for read operation. This is a bug. The gas refund constant should always be lower than the gas to charge.");
            }

            Ok(val)
        }

        fn get_decoded<V, Codec>(
            &mut self,
            storage_key: &SlotKey,
            codec: &Codec,
        ) -> Result<Option<V>, Self::Error>
        where
            Codec: StateCodec,
            Codec::ValueCodec: StateItemCodec<V>,
        {
            let storage_value = <Self as StateReader<$namespace>>::get(self, storage_key)?;

            if let Some(storage_value) = &storage_value {
                match charge_decode_gas_cost::<T::Spec>(storage_value, self) {
                    Ok(gas_cost) => gas_cost,
                    Err(e) => {
                        return Err(StateAccessorError::Decode {
                            key: storage_key.clone(),
                            inner: e,
                            namespace: <$namespace>::PROVABLE_NAMESPACE,
                        })
                    }
                };
            }

            Ok(storage_value
                .map(|storage_value| codec.value_codec().decode_unwrap(storage_value.value())))
        }
    };
}

impl<T: ProvableStateReader<Kernel>> StateReader<Kernel> for T {
    blanket_impl_metered_state_reader!(Kernel);
}

impl<T: ProvableStateReader<User>> StateReader<User> for T {
    blanket_impl_metered_state_reader!(User);
}

impl<T: AccessoryStateReader> StateReader<Accessory> for T {
    type Error = Infallible;

    /// Get a value from the storage.
    fn get(&mut self, key: &SlotKey) -> Result<Option<SlotValue>, Self::Error> {
        Ok(self.get_value(Accessory::NAMESPACE, key).0)
    }

    /// Get a decoded value from the storage.
    fn get_decoded<V, Codec>(
        &mut self,
        storage_key: &SlotKey,
        codec: &Codec,
    ) -> Result<Option<V>, Self::Error>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
    {
        let storage_value = <Self as StateReader<Accessory>>::get(self, storage_key)?;

        Ok(storage_value
            .map(|storage_value| codec.value_codec().decode_unwrap(storage_value.value())))
    }
}

/// Provides write-only access to a particular namespace
pub trait StateWriter<N: CompileTimeNamespace>: UniversalStateAccessor {
    /// The error type returned when a state write operation fails.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Sets a value in the storage. Basically a wrapper around [`StateWriter::set`].
    ///
    /// ## Error
    /// This method can fail if the gas meter doesn't have enough funds to pay for the write operation.
    fn set(&mut self, key: &SlotKey, value: SlotValue) -> Result<(), Self::Error>;

    /// Deletes a value from the storage. Basically a wrapper around [`StateWriter::delete`].
    ///
    /// ## Error
    /// This method can fail if the gas meter doesn't have enough funds to pay for the delete operation.
    fn delete(&mut self, key: &SlotKey) -> Result<(), Self::Error>;
}

/// A trait wrapper that replicates the functionality of [`StateWriter`] but with a gas metering interface.
/// This allows a storage writer to charge gas for write operations.
pub trait ProvableStateWriter<N: ProvableCompileTimeNamespace>:
    UniversalStateAccessor + GasMeter
{
}

macro_rules! blanket_impl_metered_state_writer {
    ($namespace:ty) => {
        impl<T: ProvableStateWriter<$namespace>> StateWriter<$namespace> for T {
            type Error = StateAccessorError<<T::Spec as GasSpec>::Gas>;

            fn set(&mut self, key: &SlotKey, value: SlotValue) -> Result<(), Self::Error> {

                let input_len = value.size() as u64;
                self.charge_linear_gas(&<T::Spec as GasSpec>::gas_to_charge_per_byte_for_write(), input_len).map_err(|e| StateAccessorError::Set{
                    key: key.clone(),
                    inner: e,
                    namespace: <$namespace>::PROVABLE_NAMESPACE,
                })?;

                let is_value_cached =  <Self as super::accessors::seal::UniversalStateAccessor>::set_value(
                    self,
                    <$namespace as sov_state::CompileTimeNamespace>::NAMESPACE,
                    key,
                    value,
                );

                if is_value_cached == IsValueCached::Yes {
                    let gas_to_refund = &<T::Spec as GasSpec>::gas_to_refund_per_byte_for_hot_write().checked_scalar_product(input_len).unwrap();
                    self.refund_gas(gas_to_refund).expect("Failed to refund gas for write operation. This is a bug. The gas refund constant should always be lower than the gas to charge.");
                }

                Ok(())
            }

            fn delete(&mut self, key: &SlotKey) -> Result<(), Self::Error> {
                self.charge_gas(&<T::Spec as GasSpec>::gas_to_charge_for_delete()).
                    map_err(|e|
                        StateAccessorError::Delete{
                            key: key.clone(),
                            inner: e,
                            namespace: <$namespace>::PROVABLE_NAMESPACE,
                        })?;

                        let is_value_cached =  <Self as super::accessors::seal::UniversalStateAccessor>::delete_value(
                                self,
                                <$namespace as sov_state::CompileTimeNamespace>::NAMESPACE,
                                key,
                            );

                        if is_value_cached == IsValueCached::Yes {
                            self.refund_gas(&<T::Spec as GasSpec>::gas_to_refund_for_hot_delete()).expect("Failed to refund gas for delete operation. This is a bug. The gas refund constant should always be lower than the gas to charge.");
                        }

                Ok(())
            }
        }
    };
}

blanket_impl_metered_state_writer!(User);
blanket_impl_metered_state_writer!(Kernel);

/// Provides write-only access to the accessory state
/// Does not charge gas for write/delete operations.
pub trait AccessoryStateWriter: UniversalStateAccessor {}

impl<T: AccessoryStateWriter> StateWriter<Accessory> for T {
    type Error = Infallible;

    /// Replaces a storage value.
    fn set(&mut self, key: &SlotKey, value: SlotValue) -> Result<(), Self::Error> {
        self.set_value(Accessory::NAMESPACE, key, value);
        Ok(())
    }

    /// Deletes a storage value.
    fn delete(&mut self, key: &SlotKey) -> Result<(), Self::Error> {
        self.delete_value(Accessory::NAMESPACE, key);
        Ok(())
    }
}

#[cfg(feature = "native")]
/// Allows a type to retrieve state values with a proof of their presence/absence.
pub trait ProvenStateAccessor<N: ProvableCompileTimeNamespace>: StateReaderAndWriter<N> {
    /// The underlying storage whose proof is returned
    type Proof;
    /// Fetch the value with the requested key and provide a proof of its presence/absence.
    fn get_with_proof(&mut self, key: SlotKey) -> Option<StorageProof<Self::Proof>>
    where
        Self: StateReaderAndWriter<N>,
        N: ProvableCompileTimeNamespace;
}

/// A [`StateReader`] that is version-aware.
pub trait VersionReader {
    /// Returns the largest visible slot number that the accessor is allowed to access.
    /// This may differ from the actual value of the "current" visible slot number depending on the accessor's permissions.
    // FIXME: This trait needs reworking - the number returned from this method is not always visible - that's the whole point!
    fn visible_slot_number_to_access(&self) -> VisibleSlotNumber;

    /// Returns the current version of the state accessor
    fn rollup_height_to_access(&self) -> RollupHeight;
}

/// A trait for state accessors that can write to the kernel at the true
/// [`SlotNumber`].
pub trait KernelWriter: StateWriter<namespaces::Kernel> {
    /// Returns the current true rollup height contained in the accessor
    fn true_slot_number(&self) -> SlotNumber;
}
