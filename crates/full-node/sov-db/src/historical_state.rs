use std::fmt::Debug;

use rockbound::cache::delta_reader::DeltaReader;
use rockbound::{SchemaBatch, SchemaKey, SchemaValue};
use sov_rollup_interface::common::SlotNumber;

use crate::namespaces::{KernelNamespace, Namespace, UserNamespace};
use crate::schema::namespace::StateValues;
use crate::{ensure_version_is_correct, DbOptions};

/// A typed wrapper around the [`DeltaReader`] for reading materializing historical rollup state.
#[derive(Debug, Clone)]
pub struct HistoricalStateReader {
    /// The underlying [`DeltaReader`] correctly routes requests to previous snapshots and/or [`rockbound::DB`]
    db: DeltaReader,
    /// The [`SlotNumber`] that will be used for the next batch of writes to the DB
    /// This [`SlotNumber`] is also used for querying data,
    /// so if this instance of [`HistoricalStateReader`] is used as read-only, it won't see newer data.
    next_version: SlotNumber,
}

impl HistoricalStateReader {
    const DB_PATH_SUFFIX: &'static str = "historical-state";
    const DB_NAME: &'static str = "historical-state-db";

    /// Create a new instance of [`HistoricalStateReader`] from a given [`DeltaReader`].
    pub fn with_delta_reader(reader: DeltaReader) -> anyhow::Result<Self> {
        let next_version = Self::next_version_from(&reader)?;
        Ok(Self {
            db: reader,
            next_version,
        })
    }

    /// Get the next version from the database snapshot
    fn next_version_from(reader: &DeltaReader) -> anyhow::Result<SlotNumber> {
        // Kernel updates required to always be non-empty, thus always have an higher or equal version comparing to userspace
        let kernel_last_key_value = reader.get_largest::<StateValues<KernelNamespace>>()?;
        let kernel_largest_version = kernel_last_key_value.map(|((_key, version), _)| version);

        Ok(match kernel_largest_version {
            None => SlotNumber::GENESIS,
            Some(existing_version) => existing_version
                .checked_add(1)
                .expect("State version overflow. Is is over"),
        })
    }

    /// [`DbOptions`] for [`HistoricalStateReader`].
    pub fn get_rockbound_options() -> DbOptions {
        DbOptions {
            name: Self::DB_NAME,
            path_suffix: Self::DB_PATH_SUFFIX,
            columns: UserNamespace::get_table_names()
                .into_iter()
                .chain(KernelNamespace::get_table_names())
                .collect(),
        }
    }

    /// Get the current value of the `next_version` counter
    pub fn get_next_version(&self) -> SlotNumber {
        self.next_version
    }

    /// The last version used for writes.
    pub fn last_version(&self) -> Option<SlotNumber> {
        self.next_version.checked_sub(1)
    }

    /// Get an optional value from the database, given a version and a key hash.
    pub fn get_value_option_by_key<N: Namespace>(
        &self,
        version: SlotNumber,
        key: &SchemaKey,
    ) -> anyhow::Result<Option<SchemaValue>> {
        // Defense programming
        if version >= self.next_version {
            // The future is not set.
            return Ok(None);
        }
        ensure_version_is_correct(
            key,
            version,
            self.db
                .get_prev::<StateValues<N>>(&(key.to_vec(), version))?,
        )
    }

    /// Collects a sequence of key-value pairs into [`SchemaBatch`].
    pub fn materialize_values(
        user_changes: impl IntoIterator<Item = (SchemaKey, Option<SchemaValue>)>,
        kernel_changes: impl IntoIterator<Item = (SchemaKey, Option<SchemaValue>)>,
        version: SlotNumber,
    ) -> anyhow::Result<SchemaBatch> {
        let mut batch = SchemaBatch::default();
        let mut has_kernel_been_updated = false;
        let mut has_user_been_updated = false;

        // We always .put and not .delete to keep archival data.
        for (key, value) in kernel_changes {
            batch.put::<StateValues<KernelNamespace>>(&(key, version), &value)?;
            has_kernel_been_updated = true;
        }
        for (key, value) in user_changes {
            batch.put::<StateValues<UserNamespace>>(&(key, version), &value)?;
            has_user_been_updated = true;
        }
        if has_user_been_updated && !has_kernel_been_updated {
            anyhow::bail!(
                "User namespace got updated without kernel namespace, but always should be."
            );
        }
        Ok(batch)
    }
}
