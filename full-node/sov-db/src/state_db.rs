use std::fmt::Debug;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::ensure;
use jmt::storage::{HasPreimage, TreeReader, TreeWriter};
use jmt::{KeyHash, Version};
use rockbound::cache::cache_db::CacheDb;
use rockbound::cache::change_set::ChangeSet;
use rockbound::{SchemaBatch, SchemaKey};

use crate::namespaces::{KernelNamespace, Namespace, UserNamespace};
use crate::rocks_db_config::gen_rocksdb_options;
use crate::schema::namespace::{JmtNodes, JmtValues, KeyHashToKey};

/// A typed wrapper around the db for storing rollup state. Internally,
/// this is roughly just an [`Arc<rockbound::CacheDB>`].
#[derive(Debug, Clone)]
pub struct StateDB {
    /// The underlying [`CacheDb`] that plays as local cache and pointer to previous snapshots and/or [`rockbound::DB`]
    db: Arc<CacheDb>,
    /// The [`Version`] that will be used for the next batch of writes to the DB
    /// This [`Version`] is also used for querying data,
    /// so if this instance of StateDB is used as read only, it won't see newer data.
    next_version: Arc<Mutex<Version>>,
}

impl StateDB {
    const DB_PATH_SUFFIX: &'static str = "state";
    const DB_NAME: &'static str = "state-db";

    /// Create a new instance of [`StateDB`] from a given [`rockbound::DB`]
    pub fn with_cache_db(db: CacheDb) -> anyhow::Result<Self> {
        let next_version = Self::next_version_from(&db)?;
        Ok(Self {
            db: Arc::new(db),
            next_version: Arc::new(Mutex::new(next_version)),
        })
    }

    /// Returns the associated JMT handler for a given namespace
    pub fn get_jmt_handler<N: Namespace>(&self) -> JmtHandler<N> {
        JmtHandler {
            state_db: self,
            phantom: Default::default(),
        }
    }

    /// Get the next version from the database snapshot
    fn next_version_from(db_snapshot: &CacheDb) -> anyhow::Result<Version> {
        let kernel_last_key_value = db_snapshot.get_largest::<JmtNodes<KernelNamespace>>()?;
        let kernel_largest_version = kernel_last_key_value.map(|(k, _)| k.version());

        let user_last_key_value = db_snapshot.get_largest::<JmtNodes<UserNamespace>>()?;
        let user_largest_version = user_last_key_value.map(|(k, _)| k.version());

        ensure!(
            kernel_largest_version == user_largest_version,
            "Kernel and User namespaces have different largest versions"
        );

        let next_version = user_largest_version
            .unwrap_or_default()
            .checked_add(1)
            .expect("JMT Version overflow. Is is over");

        Ok(next_version)
    }

    /// Initialize [`rockbound::DB`] that should be used by snapshots.
    /// Should initialize all the namespace tables under the same DB.
    /// Maybe we can use a macro to loop over all the namespaces.
    pub fn setup_schema_db(path: impl AsRef<Path>) -> anyhow::Result<rockbound::DB> {
        let state_db_path = path.as_ref().join(Self::DB_PATH_SUFFIX);
        rockbound::DB::open(
            state_db_path,
            Self::DB_NAME,
            UserNamespace::get_table_names()
                .into_iter()
                .chain(KernelNamespace::get_table_names()),
            &gen_rocksdb_options(&Default::default(), false),
        )
    }

    /// Convert it to [`ChangeSet`] which cannot be edited anymore
    pub fn freeze(self) -> anyhow::Result<ChangeSet> {
        let inner = Arc::into_inner(self.db).ok_or(anyhow::anyhow!(
            "StateDB underlying CacheDb has more than 1 strong references"
        ))?;
        Ok(ChangeSet::from(inner))
    }

    /// Put the preimage of a hashed key into the database. Note that the preimage is not checked for correctness,
    /// since the DB is unaware of the hash function used by the JMT.
    pub fn put_preimages<'a, N: Namespace>(
        &self,
        items: impl IntoIterator<Item = (KeyHash, &'a SchemaKey)>,
    ) -> Result<(), anyhow::Error> {
        let mut batch = SchemaBatch::new();
        for (key_hash, key) in items.into_iter() {
            batch.put::<KeyHashToKey<N>>(&key_hash.0, key)?;
        }
        self.db.write_many(batch)?;
        Ok(())
    }

    /// Increment the `next_version` counter by 1.
    pub fn inc_next_version(&self) {
        let mut version = self.next_version.lock().unwrap();
        *version += 1;
    }

    /// Get the current value of the `next_version` counter
    pub fn get_next_version(&self) -> Version {
        let version = self.next_version.lock().unwrap();
        *version
    }

    /// Get an optional value from the database, given a version and a key hash.
    pub fn get_value_option_by_key<N: Namespace>(
        &self,
        version: Version,
        key: &SchemaKey,
    ) -> anyhow::Result<Option<jmt::OwnedValue>> {
        let found = self.db.get_prev::<JmtValues<N>>(&(key, version))?;

        match found {
            Some(((found_key, found_version), value)) => {
                if &found_key == key {
                    anyhow::ensure!(found_version <= version, "Bug! iterator isn't returning expected values. expected a version <= {version:} but found {found_version:}");
                    Ok(value)
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }
}

/// A simple wrapper around [`StateDB`] that implements [`TreeReader`] and [`TreeWriter`] for a given namespace.
#[derive(Debug)]
pub struct JmtHandler<'a, N: Namespace> {
    state_db: &'a StateDB,
    phantom: std::marker::PhantomData<N>,
}

/// Default implementations of TreeReader for StateDB
impl<'a, N: Namespace> TreeReader for JmtHandler<'a, N> {
    fn get_node_option(
        &self,
        node_key: &jmt::storage::NodeKey,
    ) -> anyhow::Result<Option<jmt::storage::Node>> {
        self.state_db.db.read::<JmtNodes<N>>(node_key)
    }

    fn get_value_option(
        &self,
        version: Version,
        key_hash: KeyHash,
    ) -> anyhow::Result<Option<jmt::OwnedValue>> {
        let key_opt = self.state_db.db.read::<KeyHashToKey<N>>(&key_hash.0)?;

        if let Some(key) = key_opt {
            self.state_db.get_value_option_by_key::<N>(version, &key)
        } else {
            Ok(None)
        }
    }

    fn get_rightmost_leaf(
        &self,
    ) -> anyhow::Result<Option<(jmt::storage::NodeKey, jmt::storage::LeafNode)>> {
        todo!("StateDB does not support [`TreeReader::get_rightmost_leaf`] yet")
    }
}

/// Default implementation of TreeWriter for StateDB
impl<'a, N: Namespace> TreeWriter for JmtHandler<'a, N> {
    fn write_node_batch(&self, node_batch: &jmt::storage::NodeBatch) -> anyhow::Result<()> {
        let mut batch = SchemaBatch::new();
        for (node_key, node) in node_batch.nodes() {
            batch.put::<JmtNodes<N>>(node_key, node)?;
        }

        for ((version, key_hash), value) in node_batch.values() {
            let key_preimage =
                self
                    .state_db
                    .db
                    .read::<KeyHashToKey<N>>(&key_hash.0)?
                    .ok_or(anyhow::format_err!(
                                    "Could not find preimage for key hash {key_hash:?}. Has `StateDB::put_preimage` been called for this key?"
                                ))?;
            batch.put::<JmtValues<N>>(&(key_preimage, *version), value)?;
        }
        self.state_db.db.write_many(batch)?;
        Ok(())
    }
}

impl<'a, N: Namespace> HasPreimage for JmtHandler<'a, N> {
    fn preimage(&self, key_hash: KeyHash) -> anyhow::Result<Option<Vec<u8>>> {
        self.state_db.db.read::<KeyHashToKey<N>>(&key_hash.0)
    }
}

#[cfg(test)]
mod state_db_tests {
    use std::path;
    use std::sync::{Arc, RwLock};

    use jmt::storage::{NodeBatch, TreeReader, TreeWriter};
    use jmt::KeyHash;
    use rockbound::cache::cache_container::CacheContainer;
    use rockbound::cache::cache_db::CacheDb;
    use sha2::Sha256;

    use crate::namespaces::{KernelNamespace, Namespace, UserNamespace};
    use crate::state_db::{JmtHandler, StateDB};

    fn init_cache_db(path: &path::Path) -> CacheDb {
        let db = StateDB::setup_schema_db(path).unwrap();
        let cache_container =
            CacheContainer::new(db, Arc::new(RwLock::new(Default::default())).into());

        CacheDb::new(0, Arc::new(RwLock::new(cache_container)).into())
    }

    #[test]
    fn test_simple() {
        let tempdir = tempfile::tempdir().unwrap();
        let db_snapshot = init_cache_db(tempdir.path());
        let state_db = &StateDB::with_cache_db(db_snapshot).unwrap();
        let state_db_handler: JmtHandler<UserNamespace> = state_db.get_jmt_handler();
        let key_hash = KeyHash([1u8; 32]);
        let key = vec![2u8; 100];
        let value = [8u8; 150];

        state_db
            .put_preimages::<UserNamespace>(vec![(key_hash, &key)])
            .unwrap();
        let mut batch = NodeBatch::default();
        batch.extend(vec![], vec![((0, key_hash), Some(value.to_vec()))]);
        state_db_handler.write_node_batch(&batch).unwrap();

        let found = state_db_handler.get_value(0, key_hash).unwrap();
        assert_eq!(found, value);

        let found = state_db
            .get_value_option_by_key::<UserNamespace>(0, &key)
            .unwrap()
            .unwrap();
        assert_eq!(found, value);
    }

    #[test]
    fn test_namespace() {
        let tempdir = tempfile::tempdir().unwrap();
        let db_snapshot = init_cache_db(tempdir.path());
        let state_db = StateDB::with_cache_db(db_snapshot).unwrap();
        let user_state_db_handler: JmtHandler<'_, UserNamespace> = state_db.get_jmt_handler();
        let kernel_state_db_handler: JmtHandler<'_, KernelNamespace> = state_db.get_jmt_handler();

        // Populate the user space of the state db with some values
        {
            let key_hash = KeyHash([1u8; 32]);
            let key = vec![2u8; 100];
            let value = [8u8; 150];

            state_db
                .put_preimages::<UserNamespace>(vec![(key_hash, &key)])
                .unwrap();
            let mut batch = NodeBatch::default();
            batch.extend(vec![], vec![((0, key_hash), Some(value.to_vec()))]);
            user_state_db_handler.write_node_batch(&batch).unwrap();

            let found = user_state_db_handler.get_value(0, key_hash).unwrap();
            assert_eq!(found, value);
        }

        // Try to retrieve these values from the kernel space
        {
            let key_hash = KeyHash([1u8; 32]);

            assert!(kernel_state_db_handler.get_value(0, key_hash).is_err());
        }

        // Populate the kernel space of the state db with some values but for different version
        {
            let key_hash = KeyHash([1u8; 32]);
            let key = vec![2u8; 100];
            let value = [8u8; 150];

            state_db
                .put_preimages::<KernelNamespace>(vec![(key_hash, &key)])
                .unwrap();
            let mut batch = NodeBatch::default();
            batch.extend(vec![], vec![((1, key_hash), Some(value.to_vec()))]);
            kernel_state_db_handler.write_node_batch(&batch).unwrap();

            let found = kernel_state_db_handler.get_value(1, key_hash).unwrap();
            assert_eq!(found, value);

            assert_eq!(
                kernel_state_db_handler.get_value(1, key_hash).unwrap(),
                value
            );
        }
    }

    #[test]
    fn test_root_hash_at_init() {
        let tempdir = tempfile::tempdir().unwrap();
        let db_snapshot = init_cache_db(tempdir.path());
        let db = StateDB::with_cache_db(db_snapshot).unwrap();
        let latest_version = db.get_next_version() - 1;
        assert_eq!(0, latest_version);

        let user_state_db_handler: JmtHandler<'_, UserNamespace> = db.get_jmt_handler();
        check_root_hash_at_init_handler(&user_state_db_handler);
        let kernel_state_db_handler: JmtHandler<'_, KernelNamespace> = db.get_jmt_handler();

        check_root_hash_at_init_handler(&kernel_state_db_handler);
    }

    fn check_root_hash_at_init_handler<N: Namespace>(handler: &JmtHandler<N>) {
        let jmt = jmt::JellyfishMerkleTree::<JmtHandler<N>, Sha256>::new(handler);

        // Just pointing out the obvious.
        let root_hash = jmt.get_root_hash_option(0).unwrap();
        assert!(root_hash.is_none());
    }
}
