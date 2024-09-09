#![allow(dead_code)]
use std::marker::PhantomData;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_db::ledger_db::LedgerDb;
use sov_db::schema::types::{SlotNumber, StoredStfInfo};
use sov_db::schema::DeltaReader;
use sov_rollup_interface::da::{BlockHeaderTrait, DaSpec};
use tokio::sync::mpsc;

use crate::StateTransitionInfo;

/// Stores STF infos in the Db and sends notifications to the associated `Receiver`.
pub struct Sender<StateRoot, Witness, Da: DaSpec> {
    ledger_db: LedgerDb,
    // The notification channel does not contain the actual STF info data
    // only the indexes in the Db where the data is stored.
    notifier: mpsc::Sender<u64>,
    db: Arc<rockbound::DB>,
    _phantom: PhantomData<(StateRoot, Witness, Da)>,
}

/// Receives notifications from the associated `Sender` and reads STF info data from the db.

pub struct Receiver<StateRoot, Witness, Da: DaSpec> {
    ledger_db: LedgerDb,
    receiver: mpsc::Receiver<u64>,
    db: Arc<rockbound::DB>,
    _phantom: PhantomData<(StateRoot, Witness, Da)>,
}

/// Creates a new [`Sender`] and [`Receiver`]
/// The data in the channel is preserved between Db restarts.
pub async fn new_stf_info_channel<StateRoot, Witness, Da: DaSpec>(
    db: Arc<rockbound::DB>,
    max_channel_size: usize,
) -> anyhow::Result<(
    Sender<StateRoot, Witness, Da>,
    Receiver<StateRoot, Witness, Da>,
)> {
    // Internally the Db keeps the following entries:
    // 1. The STF info data.
    // 2. The latest height of the written STF info (increased on every save operation)
    // 3. The latest height of the retrieved STF info (increased on every `read_next`` operation).

    // On startup, we need to fill the notification channel with the pending STF info from the db.
    let (notifier, receiver) = tokio::sync::mpsc::channel::<u64>(max_channel_size);

    let reader = DeltaReader::new(db.clone(), Vec::new());
    let ledger_db = LedgerDb::with_reader(reader)?;

    let maybe_write_rollup_height = ledger_db.get_stf_info_write_rollup_height()?;
    match maybe_write_rollup_height {
        Some(write_rollup_height) => {
            let read_rollup_height = ledger_db.get_stf_info_read_rollup_height()?.unwrap_or(1);
            assert!(write_rollup_height >= read_rollup_height);
            assert!(write_rollup_height - read_rollup_height < max_channel_size as u64);

            for height in read_rollup_height..=write_rollup_height {
                // It is ok to unwrap here, as we are sure that the sender is alive.
                notifier
                    .send(height)
                    .await
                    .expect("The receiver was dropped");
            }
        }
        // Db is empty
        None => assert!(ledger_db.get_stf_info_read_rollup_height()?.is_none()),
    }

    let sender = Sender {
        ledger_db: ledger_db.clone(),
        notifier,
        db: db.clone(),
        _phantom: PhantomData,
    };

    let receiver = Receiver {
        ledger_db,
        receiver,
        db,
        _phantom: PhantomData,
    };

    Ok((sender, receiver))
}

impl<StateRoot, Witness, Da: DaSpec> Sender<StateRoot, Witness, Da>
where
    StateRoot: Serialize + DeserializeOwned,
    Witness: Serialize + DeserializeOwned,
{
    /// Saves [`StateTransitionInfo`] in the db, and and sends a notification to the [`Receiver`] that a new entry was added in the Db.
    /// This method will block if the channel is full. This can happen if the consumer of the STF info is slower than te producer.
    pub async fn save(
        &self,
        stf_info: &StateTransitionInfo<StateRoot, Witness, Da>,
    ) -> anyhow::Result<()> {
        self.save_stf_info(stf_info)?;
        self.notifier.send(stf_info.rollup_height).await?;
        Ok(())
    }

    fn save_stf_info(
        &self,
        stf_info: &StateTransitionInfo<StateRoot, Witness, Da>,
    ) -> anyhow::Result<()> {
        let encoded_stf_info: Vec<u8> = bincode::serialize(stf_info).unwrap();
        let stored_stf_info = StoredStfInfo {
            data: encoded_stf_info,
        };

        // Save the stf info in the db.
        let mut stf_info_schema_batch = self
            .ledger_db
            .materialize_stf_info(&stored_stf_info, &SlotNumber(stf_info.rollup_height))?;

        // Update the write rollup height.
        let schema_batch = self
            .ledger_db
            .materialize_stf_info_write_rollup_height(stf_info.rollup_height)?;

        stf_info_schema_batch.merge(schema_batch);

        self.db.write_schemas(&stf_info_schema_batch)?;

        Ok(())
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
            let read_rollup_height = self.get_read_rollup_height()?;

            assert_eq!(rollup_height, read_rollup_height);
            let stf_info: StateTransitionInfo<StateRoot, Witness, Da> =
                self.get(read_rollup_height)?.unwrap_or_else(|| {
                    panic!("The STF for the {} height is missing", read_rollup_height)
                });

            assert_eq!(stf_info.da_block_header().height(), read_rollup_height);
            self.inc_read_rollup_height()?;
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

    fn inc_read_rollup_height(&self) -> anyhow::Result<()> {
        let read_height = self.get_read_rollup_height()?;
        let schema_batch = self
            .ledger_db
            .materialize_stf_info_read_rollup_height(read_height + 1)?;
        self.db.write_schemas(&schema_batch)?;
        Ok(())
    }

    fn get_read_rollup_height(&self) -> anyhow::Result<u64> {
        let read_height = self.ledger_db.get_stf_info_read_rollup_height()?;
        Ok(read_height.unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use sov_mock_da::{MockBlockHeader, MockDaSpec, MockHash};
    use sov_modules_api::da::Time;
    use sov_rollup_interface::da::{DaProof, RelevantBlobs, RelevantProofs};
    use sov_rollup_interface::zk::StateTransitionWitness;

    use super::*;
    use crate::StateTransitionInfo;

    type StateRoot = Vec<u8>;
    type Witness = Vec<u8>;

    #[tokio::test]
    async fn test_stf_info_start_stop_db() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let channel_size = 10;

        // Write some data to the Db.
        {
            let db = Arc::new(new_db(temp_dir.path()));
            let (notifier, _receiver) = new_stf_info_channel(db, channel_size).await?;

            for height in 1..channel_size + 1 {
                let stf_info = make_stf_info(MockHash([height as u8; 32]), height as u64);
                notifier.save(&stf_info).await?;
            }
        }

        // After Db restart the notification mechanism works as expected.
        {
            let db = Arc::new(new_db(temp_dir.path()));
            let (_notifier, mut receiver) =
                new_stf_info_channel::<StateRoot, Witness, MockDaSpec>(db, channel_size).await?;

            for height in 1..channel_size + 1 {
                let stf_info = receiver.read_next().await?.unwrap();
                assert_eq!(stf_info.da_block_header().height, height as u64);
            }
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_stf_info_channel() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let db = Arc::new(new_db(temp_dir.path()));

        let channel_size = 10;
        let (sender, mut receiver) = new_stf_info_channel(db, channel_size).await?;

        // Fill the db.
        for height in 1..channel_size {
            let stf_info = make_stf_info(MockHash([height as u8; 32]), height as u64);
            sender.save(&stf_info).await?;
        }

        // Read the data from the db.
        for height in 1..channel_size {
            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.da_block_header().height, height as u64);
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_stf_info_drop_sender() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let db = Arc::new(new_db(temp_dir.path()));

        let channel_size = 10;
        let (sender, mut receiver) = new_stf_info_channel(db, channel_size).await?;

        for height in 1..3 {
            let stf_info = make_stf_info(MockHash([height as u8; 32]), height as u64);
            sender.save(&stf_info).await?;
        }

        drop(sender);

        for height in 1..3 {
            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.da_block_header().height, height as u64);
        }

        let stf_info = receiver.read_next().await?;
        assert!(stf_info.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_stf_info_channel_concurrent() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let db = Arc::new(new_db(temp_dir.path()));

        let channel_size = 10;
        let (sender, mut receiver) = new_stf_info_channel(db, channel_size).await?;

        tokio::spawn(async move {
            // Fill the db.
            for height in 1..channel_size {
                let stf_info = make_stf_info(MockHash([height as u8; 32]), height as u64);
                sender.save(&stf_info).await.unwrap();
            }
        });

        // Read the data from the db.
        for height in 1..channel_size {
            let stf_info = receiver.read_next().await?.unwrap();
            assert_eq!(stf_info.da_block_header().height, height as u64);
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_stf_info_in_db() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let db = Arc::new(new_db(temp_dir.path()));

        let channel_size = 10;
        let (sender, mut receiver) = new_stf_info_channel(db, channel_size).await?;

        // At the begining the db should be empty.
        let fetched_stf_info = receiver.get(1)?;
        assert!(fetched_stf_info.is_none());

        // Insert astf info two times.
        let header_hash_1 = MockHash([11; 32]);
        assert_stf_in_db(header_hash_1, 1, &sender, &mut receiver);

        let header_hash_2 = MockHash([22; 32]);
        assert_stf_in_db(header_hash_2, 2, &sender, &mut receiver);

        // Check if the first stf is still in the db.
        let fetched_stf_info = receiver.get(1)?;
        assert!(fetched_stf_info.is_some());

        Ok(())
    }

    fn assert_stf_in_db(
        header_hash: MockHash,
        rollup_height: u64,
        sender: &Sender<StateRoot, Witness, MockDaSpec>,
        receiver: &mut Receiver<StateRoot, Witness, MockDaSpec>,
    ) {
        let original_state_transition_info = make_stf_info(header_hash, rollup_height);

        sender
            .save_stf_info(&original_state_transition_info)
            .unwrap();

        let fetched_stf_info = receiver.get(rollup_height).unwrap().unwrap();

        assert_eq!(
            get_header_hash(&original_state_transition_info),
            get_header_hash(&fetched_stf_info)
        );
    }

    fn make_stf_info(
        header_hash: MockHash,
        height: u64,
    ) -> StateTransitionInfo<Vec<u8>, Vec<u8>, MockDaSpec> {
        StateTransitionInfo::new(
            StateTransitionWitness {
                initial_state_root: vec![1, 2, 3],
                final_state_root: vec![3, 4, 5],
                da_block_header: MockBlockHeader {
                    prev_hash: [0; 32].into(),
                    hash: header_hash,
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
