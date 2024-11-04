use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use sov_state::{BorshCodec, Kernel, Namespace, Prefix, StateCodec, StateItemCodec, Storage};
use unwrap_infallible::UnwrapInfallible;

use super::map::NamespacedStateMap;
use crate::{KernelStateAccessor, KernelWriter, StateReader, VersionReader};

/// A `versioned` value stored in kernel state. The semantics of this type are different
/// depending on the priveleges of the accessor. For a standard ("user space") interaction
/// via a `VersionedStateReadWriter`, only one version of this value is accessible. Inside the kernel,
/// (where access is mediated by a [`KernelStateAccessor`]), all versions of this value are accessible.
///
/// Under the hood, a versioned value is implemented as a map from a rollup height to a value. From the kernel, any
/// value can be accessed
// TODO: Automatically clear out old versions from state https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/383
#[derive(
    Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, serde::Serialize, serde::Deserialize,
)]
pub struct VersionedStateValue<V, Codec = BorshCodec> {
    _phantom: PhantomData<V>,
    elems: NamespacedStateMap<Kernel, u64, V, Codec>,
}

impl<V> VersionedStateValue<V>
where
    V: BorshSerialize,
    V: BorshDeserialize,
{
    /// Crates a new [`VersionedStateValue`] with the given prefix and the default
    /// [`StateItemCodec`] (i.e. [`BorshCodec`]).
    pub fn new(prefix: Prefix) -> Self {
        Self::with_codec(prefix, BorshCodec)
    }
}

impl<V, Codec> VersionedStateValue<V, Codec>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<u64>,
    Codec::KeyCodec: StateItemCodec<u64>,
{
    /// The namespace where the versioned state value is stored.
    pub const NAMESPACE: Namespace = Namespace::Kernel;

    /// Creates a new [`VersionedStateValue`] with the given prefix and codec.
    pub fn with_codec(prefix: Prefix, codec: Codec) -> Self {
        Self {
            _phantom: PhantomData,
            elems: NamespacedStateMap::with_codec(prefix, codec),
        }
    }

    /// Returns the prefix used when this [`VersionedStateValue`] was created.
    pub fn prefix(&self) -> &Prefix {
        &self.elems.prefix
    }
}

impl<V, Codec> VersionedStateValue<V, Codec>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<u64>,
    Codec::KeyCodec: StateItemCodec<u64>,
{
    /// Returns the codec used by the versioned state value.
    pub fn codec(&self) -> &Codec {
        self.elems.codec()
    }

    /// Any version_aware working set can read the current contents of a versioned value.
    pub fn get_current<Reader: VersionReader>(
        &self,
        state: &mut Reader,
    ) -> Result<Option<V>, <Reader as StateReader<Kernel>>::Error> {
        self.elems.get(&state.rollup_height_to_access(), state)
    }

    /// Only the kernel working set can write to versioned values
    pub fn set_true_current<Accessor: KernelWriter>(&self, value: &V, state: &mut Accessor) {
        self.elems
            .set(&state.true_rollup_height(), value, state)
            .unwrap_infallible();
    }

    /// Only the kernel working set can write to versioned values
    pub fn set<S: Storage>(&self, key: &u64, value: &V, state: &mut KernelStateAccessor<'_, S>)
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
        Codec::KeyCodec: StateItemCodec<u64>,
    {
        self.elems.set(key, value, state).unwrap_infallible();
    }

    /// Any version_aware working set can read the current contents of a versioned value.
    pub fn get<Reader>(&self, key: &u64, state: &mut Reader) -> Result<Option<V>, Reader::Error>
    where
        Reader: VersionReader,
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
        Codec::KeyCodec: StateItemCodec<u64>,
    {
        self.elems.get(key, state)
    }
}

#[cfg(test)]
mod tests {

    use sov_mock_zkvm::MockZkvm;
    use sov_rollup_interface::execution_mode::Native;
    use sov_state::Prefix;
    use sov_test_utils::storage::new_finalized_storage;
    use sov_test_utils::MockDaSpec;
    use unwrap_infallible::UnwrapInfallible;

    use crate::capabilities::mocks::MockKernel;
    use crate::runtime::capabilities::Kernel as _;
    use crate::{StateCheckpoint, VersionedStateValue};

    type TestSpec = crate::default_spec::DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, Native>;

    #[test]
    fn test_kernel_state_value_as_value() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_finalized_storage(tmpdir.path());

        let kernel = MockKernel::<TestSpec>::new(4, 1);
        let mut state = StateCheckpoint::new(storage, &kernel);

        let prefix = Prefix::new(b"test".to_vec());
        let value = VersionedStateValue::<u64>::new(prefix.clone());

        // Initialize a value in the kernel state during slot 4
        let mut kernel_state = kernel.accessor(&mut state);
        value.set_true_current(&100, &mut kernel_state);
        assert_eq!(
            value.get_current(&mut kernel_state).unwrap_infallible(),
            Some(100)
        );

        // Try to read the value from kernel space with the rollup height set to 1. Should fail.
        assert_eq!(value.get_current(&mut state).unwrap_infallible(), None);

        // Try to read the value from kernel space with the rollup height set to 4. Should succeed.
        state.update_version(4);
        assert_eq!(value.get_current(&mut state).unwrap_infallible(), Some(100));
    }

    #[test]
    fn test_kernel_state_value_as_map() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_finalized_storage(tmpdir.path());

        let kernel = MockKernel::<TestSpec>::new(4, 1);
        let mut state = StateCheckpoint::new(storage, &kernel);

        let prefix = Prefix::new(b"test".to_vec());
        let value = VersionedStateValue::<u64>::new(prefix.clone());

        // Initialize a versioned value in the kernel state to be available starting at slot 2

        let mut kernel_state = kernel.accessor(&mut state);
        value.set(&2, &100, &mut kernel_state);
        assert_eq!(
            value.get(&2, &mut kernel_state).unwrap_infallible(),
            Some(100)
        );
        value.set_true_current(&17, &mut kernel_state);

        // Try to read the value from user space with the rollup height set to 1. Should fail.
        assert_eq!(value.get_current(&mut state).unwrap_infallible(), None);

        // Try to read the value from user space with the rollup height set to 2. Should succeed.
        state.update_version(2);

        assert_eq!(value.get_current(&mut state).unwrap_infallible(), Some(100));

        // Try to read the value from user space with the rollup height set to 4. Should succeed.
        state.update_version(4);
        assert_eq!(value.get_current(&mut state).unwrap_infallible(), Some(17));
    }
}
