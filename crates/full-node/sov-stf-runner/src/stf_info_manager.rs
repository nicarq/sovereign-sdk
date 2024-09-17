#![allow(dead_code)]
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rockbound::SchemaBatch;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_db::ledger_db::LedgerDb;
use sov_db::schema::types::{SlotNumber, StoredStfInfo};
use sov_rollup_interface::da::DaSpec;
use tokio::sync::mpsc;

use crate::StateTransitionInfo;

/// Materializes STF infos and sends notifications to the associated `Receiver`.
pub struct Sender<StateRoot, Witness, Da: DaSpec> {
    // Height of the latest `StateTransitionInfo` that was read by the `Receiver`
    read_rollup_height: Arc<AtomicU64>,
    // Max number of entries we will keep in the Db, older data will be pruned.
    max_nb_of_infos_in_db: u64,

    // The notification channel, does not contain the actual STF info data
    // only the indexes in the Db where the data is stored.
    notifier: mpsc::Sender<u64>,
    _phantom: PhantomData<(StateRoot, Witness, Da)>,
}

/// Receives notifications from the associated `Sender` and reads STF info data from the db.

pub struct Receiver<StateRoot, Witness, Da: DaSpec> {
    // Height of the latest `StateTransitionInfo` that was read by the `Receiver`
    read_rollup_height: Arc<AtomicU64>,
    ledger_db: LedgerDb,
    receiver: mpsc::Receiver<u64>,
    _phantom: PhantomData<(StateRoot, Witness, Da)>,
}

/// Creates a new [Sender] and [Receiver] channel.
///
/// The channel's data is retained across Db restarts.
/// - The sender will block if the channel reaches `max_channel_size` of STF infos.
/// - If the number of entries in the Db exceeds `max_nb_of_infos_in_db`, the oldest data will be pruned.
/// The channel can only be created if `max_channel_size` is less than or equal to `max_nb_of_infos_in_db``.
pub async fn new_stf_info_channel<StateRoot, Witness, Da: DaSpec>(
    ledger_db: LedgerDb,
    max_channel_size: usize,
    max_nb_of_infos_in_db: u64,
) -> anyhow::Result<(
    Sender<StateRoot, Witness, Da>,
    Receiver<StateRoot, Witness, Da>,
)> {
    assert!(
        max_channel_size <= max_nb_of_infos_in_db as usize,
        "Channel size should be smaller than the max number of STFInfos in the db"
    );

    // Internally the Db keeps the following entries:
    // 1. The STF info data.
    // 2. The latest height of the written STF info (increased on every `materialize_stf_info`` operation)
    // 3. The latest height of the retrieved STF info (increased on every `read_next`` operation).

    // On startup, we need to fill the notification channel with the pending STF info from the db.
    let (notifier, receiver) = tokio::sync::mpsc::channel::<u64>(max_channel_size);

    let maybe_write_rollup_height = ledger_db.get_stf_info_write_rollup_height().await?;
    match maybe_write_rollup_height {
        Some(write_rollup_height) => {
            let read_rollup_height = ledger_db
                .get_stf_info_read_rollup_height()
                .await?
                .unwrap_or(1);
            // Sanity check for `write_rollup_height & read_rollup_height`
            assert!(
                write_rollup_height >= read_rollup_height,
                "The `write_rollup_height` should always be greater than the `read_rollup_height`"
            );
            assert!(
                write_rollup_height - read_rollup_height <= max_nb_of_infos_in_db,
                "Too many STF infos in the db"
            );

            for height in read_rollup_height..=write_rollup_height {
                // It is ok to unwrap here, as we are sure that the sender is alive.
                notifier
                    .send(height)
                    .await
                    .expect("The receiver was dropped");
            }
        }
        // Db is empty
        None => {
            assert!(ledger_db.get_stf_info_read_rollup_height().await?.is_none());
            assert!(ledger_db
                .get_stf_info_oldest_rollup_height()
                .await?
                .is_none());
        }
    }

    let read_rollup_height = Arc::new(AtomicU64::new(
        ledger_db
            .get_stf_info_read_rollup_height()
            .await?
            .unwrap_or(1),
    ));

    let sender = Sender {
        max_nb_of_infos_in_db,
        read_rollup_height: read_rollup_height.clone(),
        notifier,

        _phantom: PhantomData,
    };

    let receiver = Receiver {
        read_rollup_height,
        ledger_db,
        receiver,
        _phantom: PhantomData,
    };

    Ok((sender, receiver))
}

impl<StateRoot, Witness, Da: DaSpec> Sender<StateRoot, Witness, Da>
where
    StateRoot: Serialize + DeserializeOwned,
    Witness: Serialize + DeserializeOwned,
{
    /// Materialized [`StateTransitionInfo`], and and sends a notification to the [`Receiver`] that a new entry was added in the Db.
    /// This method will block if the channel is full. This can happen if the consumer of the STF info is slower than te producer.
    pub async fn materialize_stf_info(
        &self,
        stf_info: &StateTransitionInfo<StateRoot, Witness, Da>,
        ledger_db: &LedgerDb,
    ) -> anyhow::Result<SchemaBatch> {
        let encoded_stf_info: Vec<u8> = bincode::serialize(stf_info).unwrap();
        let stored_stf_info = StoredStfInfo {
            data: encoded_stf_info,
        };

        let write_rollup_height = stf_info.rollup_height;

        // Save the stf info in the db.
        let mut schema =
            ledger_db.materialize_stf_info(&stored_stf_info, &SlotNumber(write_rollup_height))?;

        // Update the write rollup height.
        schema.merge(ledger_db.materialize_stf_info_write_rollup_height(write_rollup_height)?);

        // Update the read rollup height.
        let read_rollup_height = self.read_rollup_height.load(Ordering::SeqCst);
        schema.merge(ledger_db.materialize_stf_info_read_rollup_height(read_rollup_height)?);

        // Prune the oldest entries if needed
        let mut oldest_height = self.get_oldest_rollup_height(ledger_db).await?;

        while Some(oldest_height) < write_rollup_height.checked_sub(self.max_nb_of_infos_in_db) {
            schema.merge(self.remove_oldest_height(oldest_height, ledger_db)?);
            oldest_height += 1;
        }

        Ok(schema)
    }

    /// Notify the `Receiver` that the data for `rollup_height` is saved in the Db.
    pub async fn notify(&self, rollup_height: u64) -> anyhow::Result<()> {
        self.notifier.send(rollup_height).await?;
        Ok(())
    }

    async fn get_oldest_rollup_height(&self, ledger_db: &LedgerDb) -> anyhow::Result<u64> {
        let oldest_height = ledger_db.get_stf_info_oldest_rollup_height().await?;
        Ok(oldest_height.unwrap_or(1))
    }

    fn remove_oldest_height(
        &self,
        oldest_height: u64,
        ledger_db: &LedgerDb,
    ) -> anyhow::Result<SchemaBatch> {
        let mut schema_batch = ledger_db.delete_stf_info(oldest_height)?;

        let inc_oldest_height_schema_batch =
            ledger_db.materialize_stf_info_oldest_rollup_height(oldest_height + 1)?;

        schema_batch.merge(inc_oldest_height_schema_batch);

        Ok(schema_batch)
    }
}

impl<StateRoot, Witness, Da: DaSpec> Receiver<StateRoot, Witness, Da>
where
    StateRoot: Serialize + DeserializeOwned,
    Witness: Serialize + DeserializeOwned,
{
    /// Reads the next [`StateTransitionInfo`] from the Db.
    /// This method will block if the channel is empty. This can happen if the producer of the STF info is slower than te consumer.
    /// Returns `Ok(None)` if the producer of the STF info was dropped.
    pub async fn read_next(
        &mut self,
    ) -> anyhow::Result<Option<StateTransitionInfo<StateRoot, Witness, Da>>> {
        if let Some(rollup_height) = self.receiver.recv().await {
            let read_rollup_height = self.read_rollup_height.load(Ordering::SeqCst);

            assert_eq!(rollup_height, read_rollup_height);
            let stf_info: StateTransitionInfo<StateRoot, Witness, Da> =
                self.get(read_rollup_height)?.unwrap_or_else(|| {
                    panic!("The STF for the {} height is missing", read_rollup_height)
                });

            assert_eq!(stf_info.rollup_height, read_rollup_height);
            self.read_rollup_height.fetch_add(1, Ordering::SeqCst);

            Ok(Some(stf_info))
        } else {
            Ok(None)
        }
    }

    /// Gets [`StateTransitionInfo`] for the corresponding rollup height.
    pub fn get(
        &self,
        rollup_height: u64,
    ) -> anyhow::Result<Option<StateTransitionInfo<StateRoot, Witness, Da>>> {
        let maybe_stored_stf_info = self.ledger_db.get_stf_info(&SlotNumber(rollup_height))?;

        if let Some(stored_stf_info) = maybe_stored_stf_info {
            Ok(Some(bincode::deserialize(&stored_stf_info.data[..])?))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sov_mock_da::{MockBlockHeader, MockDaSpec, MockHash};
    use sov_modules_api::da::Time;
    use sov_rollup_interface::da::{DaProof, RelevantBlobs, RelevantProofs};
    use sov_rollup_interface::zk::StateTransitionWitness;
    use sov_test_utils::storage::SimpleLedgerStorageManager;

    use super::*;
    use crate::StateTransitionInfo;

    type StateRoot = Vec<u8>;
    type Witness = Vec<u8>;

    async fn setup(
        path: &Path,
        max_channel_size: usize,
        max_nb_of_infos_in_db: u64,
    ) -> anyhow::Result<(
        LedgerDb,
        SimpleLedgerStorageManager,
        Sender<StateRoot, Witness, MockDaSpec>,
        Receiver<StateRoot, Witness, MockDaSpec>,
    )> {
        let mut storage_manager = SimpleLedgerStorageManager::new(path);
        let ledger_db = LedgerDb::with_reader(storage_manager.create_ledger_storage()).unwrap();

        let (sender, receiver) = new_stf_info_channel::<StateRoot, Witness, MockDaSpec>(
            ledger_db.clone(),
            max_channel_size,
            max_nb_of_infos_in_db,
        )
        .await?;

        Ok((ledger_db, storage_manager, sender, receiver))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_stf_info_start_stop_db() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let channel_size: usize = 10;
        let max_nb_of_infos_in_db = 10;

        // Write some data to the Db.
        {
            let (ledger_db, mut storage_manager, sender, _receiver) =
                setup(temp_dir.path(), channel_size, max_nb_of_infos_in_db).await?;

            for height in 1..channel_size + 1 {
                let stf_info = make_stf_info(height as u64);
                let schema_batch = sender.materialize_stf_info(&stf_info, &ledger_db).await?;
                sender.notify(stf_info.rollup_height).await?;
                storage_manager.commit(schema_batch);
            }
        }

        // Restart the Db and check that we can read the previously written data.
        {
            let (ledger_db, _, sender, mut receiver) =
                setup(temp_dir.path(), channel_size, max_nb_of_infos_in_db).await?;

            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.rollup_height, 1);
            assert_eq!(sender.get_oldest_rollup_height(&ledger_db).await?, 1);

            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.rollup_height, 2);
            assert_eq!(sender.get_oldest_rollup_height(&ledger_db).await?, 1);
        }

        // We haven't committed the reads above so after restart we will read the same data.
        {
            let (ledger_db, mut storage_manager, sender, mut receiver) =
                setup(temp_dir.path(), channel_size, max_nb_of_infos_in_db).await?;

            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.rollup_height, 1);
            assert_eq!(sender.get_oldest_rollup_height(&ledger_db).await?, 1);

            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.rollup_height, 2);
            assert_eq!(sender.get_oldest_rollup_height(&ledger_db).await?, 1);

            // Commit new data.
            let stf_info = make_stf_info((channel_size + 1) as u64);
            let schema_batch = sender.materialize_stf_info(&stf_info, &ledger_db).await?;
            storage_manager.commit(schema_batch);
            sender.notify(stf_info.rollup_height).await?;

            let stf_info = make_stf_info((channel_size + 2) as u64);
            let schema_batch = sender.materialize_stf_info(&stf_info, &ledger_db).await?;
            storage_manager.commit(schema_batch);
            sender.notify(stf_info.rollup_height).await?;

            assert_eq!(sender.get_oldest_rollup_height(&ledger_db).await?, 2);
        }

        // Now the reads are visible.
        {
            let (ledger_db, _, sender, mut receiver) =
                setup(temp_dir.path(), channel_size, max_nb_of_infos_in_db).await?;

            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.rollup_height, 3);
            assert_eq!(sender.get_oldest_rollup_height(&ledger_db).await?, 2);
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_stf_info_channel() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let channel_size = 10;
        let max_nb_of_infos_in_db = 100;

        let (ledger_db, mut storage_manager, sender, mut receiver) =
            setup(temp_dir.path(), channel_size, max_nb_of_infos_in_db).await?;

        // Fill the db.
        for height in 1..channel_size {
            let stf_info = make_stf_info(height as u64);
            let schema_batch = sender.materialize_stf_info(&stf_info, &ledger_db).await?;
            storage_manager.commit(schema_batch);
            sender.notify(stf_info.rollup_height).await?;
        }

        // Read the data from the db.
        for height in 1..channel_size {
            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.rollup_height, height as u64);
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_stf_info_drop_sender() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let channel_size = 10;
        let max_nb_of_infos_in_db = 100;

        let (ledger_db, mut storage_manager, sender, mut receiver) =
            setup(temp_dir.path(), channel_size, max_nb_of_infos_in_db).await?;

        for height in 1..3 {
            let stf_info = make_stf_info(height as u64);
            let schema_batch = sender.materialize_stf_info(&stf_info, &ledger_db).await?;
            storage_manager.commit(schema_batch);
            sender.notify(stf_info.rollup_height).await?;
        }

        drop(sender);

        for height in 1..3 {
            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.rollup_height, height as u64);
        }

        let stf_info = receiver.read_next().await?;
        assert!(stf_info.is_none());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_stf_info_channel_concurrent() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let channel_size = 10;
        let max_nb_of_infos_in_db = 100;

        let (ledger_db, mut storage_manager, sender, mut receiver) =
            setup(temp_dir.path(), channel_size, max_nb_of_infos_in_db).await?;

        tokio::spawn(async move {
            // Fill the db.
            for height in 1..channel_size {
                let stf_info = make_stf_info(height as u64);
                let schema_batch = sender
                    .materialize_stf_info(&stf_info, &ledger_db)
                    .await
                    .unwrap();
                storage_manager.commit(schema_batch);
                sender.notify(stf_info.rollup_height).await.unwrap();
            }
        });

        // Read the data from the db.
        for height in 1..channel_size {
            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.rollup_height, height as u64);
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_stf_info_in_db() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let channel_size = 10;
        let max_nb_of_infos_in_db = 100;

        let (ledger_db, mut storage_manager, sender, mut receiver) =
            setup(temp_dir.path(), channel_size, max_nb_of_infos_in_db).await?;

        // At the begining the db should be empty.
        let fetched_stf_info = receiver.get(1)?;
        assert!(fetched_stf_info.is_none());

        // Insert astf info two times.
        assert_stf_in_db(1, &sender, &mut receiver, &mut storage_manager, &ledger_db).await;

        assert_stf_in_db(2, &sender, &mut receiver, &mut storage_manager, &ledger_db).await;

        // Check if the first stf is still in the db.
        let fetched_stf_info = receiver.get(1)?;
        assert!(fetched_stf_info.is_some());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_stf_info_db_pruning() -> anyhow::Result<()> {
        struct TestCase {
            channel_size: usize,
            max_nb_of_infos_in_db: u64,
            nb_of_stf_infos: u64,
        }

        let test_cases = vec![
            TestCase {
                channel_size: 15,
                max_nb_of_infos_in_db: 20,
                nb_of_stf_infos: 30,
            },
            TestCase {
                channel_size: 15,
                max_nb_of_infos_in_db: 15,
                nb_of_stf_infos: 30,
            },
            TestCase {
                channel_size: 1,
                max_nb_of_infos_in_db: 1,
                nb_of_stf_infos: 2,
            },
        ];

        for test_case in test_cases {
            let temp_dir = tempfile::tempdir()?;

            let expected_oldest_height = std::cmp::max(
                test_case.nb_of_stf_infos - test_case.max_nb_of_infos_in_db - 1,
                1,
            );

            let (ledger_db, mut storage_manager, sender, mut receiver) = setup(
                temp_dir.path(),
                test_case.channel_size,
                test_case.max_nb_of_infos_in_db,
            )
            .await?;

            // Fill the db.
            for height in 1..test_case.nb_of_stf_infos {
                let stf_info = make_stf_info(height);
                let schema_batch = sender.materialize_stf_info(&stf_info, &ledger_db).await?;
                storage_manager.commit(schema_batch);
                sender.notify(stf_info.rollup_height).await?;
                receiver.read_next().await?.unwrap();
            }

            let oldest_height = sender.get_oldest_rollup_height(&ledger_db).await?;
            assert_eq!(oldest_height, expected_oldest_height);

            // Check if the old STF infos are pruned.
            for height in 1..test_case.nb_of_stf_infos {
                let stf_info: Option<StateTransitionInfo<Vec<u8>, Vec<u8>, MockDaSpec>> =
                    receiver.get(height)?;

                if height < oldest_height {
                    // The old data was deleted from the Db.
                    assert!(stf_info.is_none());
                } else {
                    assert!(stf_info.is_some());
                }
            }
        }
        Ok(())
    }

    async fn assert_stf_in_db(
        rollup_height: u64,
        sender: &Sender<StateRoot, Witness, MockDaSpec>,
        receiver: &mut Receiver<StateRoot, Witness, MockDaSpec>,
        storage_manager: &mut SimpleLedgerStorageManager,
        ledger_db: &LedgerDb,
    ) {
        let original_state_transition_info = make_stf_info(rollup_height);

        let schema_batch = sender
            .materialize_stf_info(&original_state_transition_info, ledger_db)
            .await
            .unwrap();
        storage_manager.commit(schema_batch);
        sender.notify(rollup_height).await.unwrap();

        let fetched_stf_info = receiver.get(rollup_height).unwrap().unwrap();

        assert_eq!(
            get_header_hash(&original_state_transition_info),
            get_header_hash(&fetched_stf_info)
        );
    }

    fn make_stf_info(height: u64) -> StateTransitionInfo<Vec<u8>, Vec<u8>, MockDaSpec> {
        StateTransitionInfo::new(
            StateTransitionWitness {
                initial_state_root: vec![1, 2, 3],
                final_state_root: vec![3, 4, 5],
                da_block_header: MockBlockHeader {
                    prev_hash: [0; 32].into(),
                    hash: MockHash([height as u8; 32]),
                    height,
                    time: Time::now(),
                },
                relevant_proofs: RelevantProofs {
                    batch: DaProof {
                        inclusion_proof: Default::default(),
                        completeness_proof: Default::default(),
                    },
                    proof: DaProof {
                        inclusion_proof: Default::default(),
                        completeness_proof: Default::default(),
                    },
                },
                relevant_blobs: RelevantBlobs {
                    proof_blobs: vec![],
                    batch_blobs: vec![],
                },
                witness: vec![],
            },
            height,
        )
    }

    fn get_header_hash(stf_info: &StateTransitionInfo<Vec<u8>, Vec<u8>, MockDaSpec>) -> MockHash {
        stf_info.da_block_header().hash
    }

    fn new_db(path: impl AsRef<std::path::Path>) -> rockbound::DB {
        LedgerDb::get_rockbound_options()
            .default_setup_db_in_path(path.as_ref())
            .unwrap()
    }
}
