use sov_modules_core::namespaces::Accessory;
use sov_state::codec::BorshCodec;

use super::vec::GenericStateVec;

/// A variant of [`StateVec`](crate::StateVec) that stores its elements as
/// "accessory" state, instead of in the JMT.
pub type AccessoryStateVec<V, Codec = BorshCodec> = GenericStateVec<Accessory, V, Codec>;

#[cfg(all(test, feature = "native"))]
mod test {

    use sov_modules_core::{Prefix, WorkingSet};
    use sov_prover_storage_manager::new_orphan_storage;
    use sov_test_utils::TestSpec;

    use super::*;
    use crate::containers::traits::vec_tests::Testable;

    #[test]
    fn test_accessory_state_vec() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_orphan_storage(tmpdir.path()).unwrap();
        let mut working_set: WorkingSet<TestSpec> = WorkingSet::new(storage);

        let prefix = Prefix::new("test".as_bytes().to_vec());
        let state_vec = AccessoryStateVec::<u32>::new(prefix);
        state_vec.run_tests(&mut working_set.accessory_state())
    }
}
