use std::collections::HashSet;

use jmt::{JellyfishMerkleTree, KeyHash, SimpleHasher};
use rand::{Rng, SeedableRng};
use rockbound::SchemaValue;

use crate::accessory_db::AccessoryDb;
use crate::namespaces::Namespace;
use crate::state_db::{JmtHandler, StateDb, StateTreeChanges};
use crate::storage_manager::InitializableNativeStorage;

/// Simple container for unlocking testing of NativeStorage without need of ProverStorage.
#[derive(Debug, Clone)]
pub struct TestNativeStorage {
    #[allow(missing_docs)]
    pub state: StateDb,
    #[allow(missing_docs)]
    pub accessory_db: AccessoryDb,
}

impl InitializableNativeStorage for TestNativeStorage {
    fn new(db: StateDb, accessory_db: AccessoryDb) -> Self {
        Self {
            state: db,
            accessory_db,
        }
    }
}

#[cfg(test)]
pub type H = sha2::Sha256;

/// Simple container fo unlocking testing of NomtStorageManager without relying on sov-state.
#[cfg(test)]
#[allow(missing_docs)]
pub struct TestNomtStorage {
    pub state_session: crate::state_db_nomt::StateSession<H>,
    pub historical_state: crate::historical_state::HistoricalStateReader,
    pub accessory_db: AccessoryDb,
}

#[cfg(test)]
impl crate::storage_manager::InitializableNativeNomtStorage<H> for TestNomtStorage {
    fn new(
        state_db: crate::state_db_nomt::StateSession<H>,
        historical_state: crate::historical_state::HistoricalStateReader,
        accessory_db: AccessoryDb,
    ) -> Self {
        TestNomtStorage {
            state_session: state_db,
            historical_state,
            accessory_db,
        }
    }
}

#[allow(missing_docs)]
pub fn generate_random_bytes(count: usize) -> HashSet<Vec<u8>> {
    let seed: [u8; 32] = [1; 32];

    // Create an RNG with the specified seed, so tests are reproducible.
    // We don't need actual randomness, we need some value distribution.
    let mut rng = rand::prelude::StdRng::from_seed(seed);

    generate_more_random_bytes(&mut rng, count, &HashSet::new())
}

/// Generates more unique keys, which are also not present in given keys.
pub fn generate_more_random_bytes<R: Rng>(
    rng: &mut R,
    count: usize,
    existing_keys: &HashSet<Vec<u8>>,
) -> HashSet<Vec<u8>> {
    let mut samples: HashSet<Vec<u8>> = HashSet::with_capacity(count);

    while samples.len() < count {
        let inner_vec_size = rng.gen_range(32..=256);
        let storage_key: Vec<u8> = (0..inner_vec_size).map(|_| rng.gen::<u8>()).collect();
        if !existing_keys.contains(&storage_key) {
            samples.insert(storage_key);
        }
    }
    samples
}

/// Helper for building proper [`StateTreeChanges`]
pub fn build_data_to_materialize<N: Namespace, H: SimpleHasher>(
    jmt_handler: &JmtHandler<N>,
    next_version: jmt::Version,
    batch: Vec<(KeyHash, Option<SchemaValue>)>,
) -> StateTreeChanges {
    let jmt = JellyfishMerkleTree::<JmtHandler<N>, H>::new(jmt_handler);
    let (_new_root, _update_proof, tree_update) =
        jmt.put_value_set_with_proof(batch, next_version).unwrap();

    StateTreeChanges {
        original_write_values: tree_update.node_batch.values().clone(),
        node_batch: tree_update.node_batch,
    }
}
