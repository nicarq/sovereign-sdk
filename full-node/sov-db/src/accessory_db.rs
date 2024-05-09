use std::sync::Arc;

use rockbound::cache::cache_db::CacheDb;
use rockbound::cache::change_set::ChangeSet;
use rockbound::SchemaBatch;

use crate::schema::tables::{ModuleAccessoryState, ACCESSORY_TABLES};
use crate::schema::types::AccessoryKey;
use crate::DbOptions;

/// Specifies a particular version of the Accessory state.
pub type Version = u64;

/// Typesafe wrapper for Data, that is not part of the provable state
#[derive(Clone, Debug)]
pub struct AccessoryDb {
    /// Pointer to [`CacheDb`] for up to date state
    db: Arc<CacheDb>,
}

impl AccessoryDb {
    const DB_PATH_SUFFIX: &'static str = "accessory";
    const DB_NAME: &'static str = "accessory-db";

    /// Get [`DbOptions`] for [`AccessoryDb`]
    pub fn get_rockbound_options() -> DbOptions {
        DbOptions {
            name: Self::DB_NAME,
            path_suffix: Self::DB_PATH_SUFFIX,
            columns: ACCESSORY_TABLES.to_vec(),
        }
    }

    /// Convert it to [`ChangeSet`] which cannot be edited anymore
    pub fn freeze(self) -> anyhow::Result<ChangeSet> {
        let inner = Arc::into_inner(self.db).ok_or(anyhow::anyhow!(
            "AccessoryDB's underlying DbSnapshot has more than 1 strong references"
        ))?;
        Ok(ChangeSet::from(inner))
    }

    /// Create instance of [`AccessoryDb`] from [`CacheDb`]
    pub fn with_cache_db(db: CacheDb) -> anyhow::Result<Self> {
        // We keep Result type, just for future archival state integration
        Ok(Self { db: Arc::new(db) })
    }

    /// Queries for a value in the [`AccessoryDb`], given a key.
    pub fn get_value_option(
        &self,
        key: &AccessoryKey,
        version: Version,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let found = self
            .db
            .get_prev::<ModuleAccessoryState>(&(key.to_vec(), version))?;
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

    /// Sets a sequence of key-value pairs in the [`AccessoryDb`]. The write is atomic.
    pub fn set_values(
        &self,
        key_value_pairs: impl IntoIterator<Item = (Vec<u8>, Option<Vec<u8>>)>,
        version: Version,
    ) -> anyhow::Result<()> {
        let mut batch = SchemaBatch::default();
        for (key, value) in key_value_pairs {
            batch.put::<ModuleAccessoryState>(&(key, version), &value)?;
        }
        self.db.write_many(batch)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::RwLock;

    use rockbound::cache::cache_container::CacheContainer;
    use rockbound::cache::cache_db::CacheDb;

    use super::*;

    fn setup_db(path: &Path) -> AccessoryDb {
        let db = AccessoryDb::get_rockbound_options()
            .default_setup_db_in_path(path)
            .unwrap();
        let to_parent = Arc::new(RwLock::new(HashMap::new()));
        let cache_container = Arc::new(RwLock::new(CacheContainer::new(
            db,
            to_parent.clone().into(),
        )));
        let db_snapshot = CacheDb::new(0, cache_container.into());
        AccessoryDb::with_cache_db(db_snapshot).unwrap()
    }

    #[test]
    fn get_after_set() {
        let tempdir = tempfile::tempdir().unwrap();
        let db = setup_db(tempdir.path());

        let key = b"foo".to_vec();
        let value = b"bar".to_vec();
        db.set_values(vec![(key.clone(), Some(value.clone()))], 0)
            .unwrap();
        assert_eq!(db.get_value_option(&key, 0).unwrap(), Some(value.clone()));
        let value2 = b"bar2".to_vec();
        db.set_values(vec![(key.clone(), Some(value2.clone()))], 1)
            .unwrap();
        assert_eq!(db.get_value_option(&key, 0).unwrap(), Some(value));
    }

    #[test]
    fn get_after_delete() {
        let tempdir = tempfile::tempdir().unwrap();
        let db = setup_db(tempdir.path());

        let key = b"deleted".to_vec();
        db.set_values(vec![(key.clone(), None)], 0).unwrap();
        assert_eq!(db.get_value_option(&key, 0).unwrap(), None);
    }

    #[test]
    fn get_nonexistent() {
        let tempdir = tempfile::tempdir().unwrap();
        let db = setup_db(tempdir.path());

        let key = b"spam".to_vec();
        assert_eq!(db.get_value_option(&key, 0).unwrap(), None);
    }
}
