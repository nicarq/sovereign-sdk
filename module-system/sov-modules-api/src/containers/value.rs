use std::marker::PhantomData;

use sov_state::codec::BorshCodec;
use sov_state::namespaces::{Accessory, CompileTimeNamespace, Kernel, User};
use sov_state::{Prefix, SlotKey, SlotValue, StateCodec, StateItemCodec};
use thiserror::Error;

use crate::{StateReader, StateReaderAndWriter, StateWriter};

/// Container for a single value.
#[derive(
    Debug,
    Clone,
    PartialEq,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct NamespacedStateValue<N, V, Codec = BorshCodec> {
    _phantom: PhantomData<(V, N)>,
    pub(crate) codec: Codec,
    pub(crate) prefix: Prefix,
}

/// Error type for getters from state values method.
#[derive(Debug, Error)]
pub enum StateValueError<N: CompileTimeNamespace> {
    #[error("Value not found for prefix: {0} in namespace: {}", std::any::type_name::<N>())]
    MissingValue(Prefix, PhantomData<N>),
}

/// A container for a single user-space value.
pub type StateValue<V, Codec = BorshCodec> = NamespacedStateValue<User, V, Codec>;
/// A Container for a single value which is only accesible in the kernel.
pub type KernelStateValue<V, Codec = BorshCodec> = NamespacedStateValue<Kernel, V, Codec>;
/// A Container for a single value stored as "accessory" state, outside of the
/// JMT.
pub type AccessoryStateValue<V, Codec = BorshCodec> = NamespacedStateValue<Accessory, V, Codec>;

// Implement a new function that assumes the BorshCodec
impl<N: CompileTimeNamespace, V> NamespacedStateValue<N, V>
where
    <BorshCodec as StateCodec>::ValueCodec: StateItemCodec<V>,
{
    /// Crates a new [`StateValue`] with the given prefix and the default
    /// [`crate::StateItemCodec`] (i.e. [`BorshCodec`]).
    pub fn new(prefix: Prefix) -> Self {
        Self::with_codec(prefix, BorshCodec)
    }
}

// Implement all other functions generically over codecs
impl<N, V, Codec> NamespacedStateValue<N, V, Codec>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V>,
    N: CompileTimeNamespace,
{
    /// Creates a new [`StateValue`] with the given prefix and codec.
    pub fn with_codec(prefix: Prefix, codec: Codec) -> Self {
        Self {
            _phantom: PhantomData,
            codec,
            prefix,
        }
    }

    pub fn prefix(&self) -> &Prefix {
        &self.prefix
    }

    /// Returns the codec used for this value
    pub fn codec(&self) -> &Codec {
        &self.codec
    }

    /// Returns the `SlotKey` for this value
    pub fn slot_key(&self) -> SlotKey {
        SlotKey::singleton(self.prefix())
    }

    /// Encodes the provided value into a slot value
    fn slot_value(&self, value: &V) -> SlotValue
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
    {
        SlotValue::new(value, self.codec().value_codec())
    }

    /// Sets the value.
    pub fn set(&self, value: &V, working_set: &mut impl StateWriter<N>) {
        working_set
            .set(&self.slot_key(), self.slot_value(value))
            .unwrap();
    }

    /// Gets the value from state or returns None if the value is absent.
    pub fn get(&self, working_set: &mut impl StateReader<N>) -> Option<V> {
        working_set
            .get_decoded(&self.slot_key(), self.codec())
            .unwrap()
    }

    /// Gets the value from state or Error if the value is absent.
    pub fn get_or_err(
        &self,
        working_set: &mut impl StateReader<N>,
    ) -> Result<V, StateValueError<N>> {
        self.get(working_set)
            .ok_or_else(|| StateValueError::<N>::MissingValue(self.prefix().clone(), PhantomData))
    }

    /// Removes the value from state, returning the value (or None if the key is absent).
    pub fn remove(&self, working_set: &mut impl StateReaderAndWriter<N>) -> Option<V> {
        working_set
            .remove_decoded(&self.slot_key(), self.codec())
            .unwrap()
    }

    /// Removes a value from state, returning the value (or Error if the key is absent).
    pub fn remove_or_err(
        &self,
        working_set: &mut impl StateReaderAndWriter<N>,
    ) -> Result<V, StateValueError<N>> {
        self.remove(working_set)
            .ok_or_else(|| StateValueError::<N>::MissingValue(self.prefix().clone(), PhantomData))
    }

    /// Deletes a value from state.
    pub fn delete(&self, working_set: &mut impl StateWriter<N>) {
        working_set.delete(&self.slot_key()).unwrap();
    }
}

#[cfg(feature = "native")]
mod proofs {
    use sov_state::namespaces::ProvableCompileTimeNamespace;
    use sov_state::{StateCodec, StateItemCodec, StateItemDecoder, Storage};

    use super::NamespacedStateValue;
    use crate::{ProvenStateAccessor, Spec};

    impl<N, V, Codec> NamespacedStateValue<N, V, Codec>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
        N: ProvableCompileTimeNamespace,
    {
        /// Gets the value with a proof of correctness.
        pub fn get_with_proof<W>(&self, working_set: &mut W) -> sov_state::StorageProof<W::Proof>
        where
            W: ProvenStateAccessor<N>,
        {
            working_set.get_with_proof(self.slot_key())
        }

        pub fn verify_proof<S: Spec>(
            &self,
            state_root: <S::Storage as Storage>::Root,
            proof: sov_state::StorageProof<<<S as Spec>::Storage as Storage>::Proof>,
        ) -> Result<Option<V>, anyhow::Error>
where {
            anyhow::ensure!(
                proof.namespace == N::PROVABLE_NAMESPACE,
                "The provided proof comes from a different namespace. Expected {:?} but found {:?}",
                N::PROVABLE_NAMESPACE,
                proof.namespace
            );

            let (key, value) = S::Storage::open_proof(state_root, proof)?;
            anyhow::ensure!(
                key == self.slot_key(),
                "The provided proof is for a different key. Expected {:?} but found {:?}",
                self.slot_key(),
                key
            );

            value
                .map(|v| {
                    self.codec()
                        .value_codec()
                        .try_decode(v.value())
                        .map_err(|e| anyhow::anyhow!("Failed to decode value from proof: {:?}", e))
                })
                .transpose()
        }
    }
}
