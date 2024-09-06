#![allow(dead_code)]
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_db::ledger_db::LedgerDb;
use sov_db::schema::types::{SlotNumber, StoredStfInfo};
use sov_db::schema::SchemaBatch;
use sov_rollup_interface::da::DaSpec;

use crate::StateTransitionInfo;

/// Manages the storage of [`StoredStfInfo`] in the ledger db.
pub(crate) struct StfInfoManager<StateRoot, Witness, Da: DaSpec> {
    ledger_db: LedgerDb,
    _phantom: PhantomData<(StateRoot, Witness, Da)>,
}

impl<StateRoot, Witness, Da: DaSpec> StfInfoManager<StateRoot, Witness, Da>
where
    StateRoot: Serialize + DeserializeOwned,
    Witness: Serialize + DeserializeOwned,
{
    /// Creates a new [`StfInfoManager`]
    pub(crate) fn new(ledger_db: LedgerDb) -> Self {
        Self {
            ledger_db,
            _phantom: PhantomData,
        }
    }

    /// Puts [`StateTransitionInfo`] into [`SchemaBatch`].
    pub(crate) fn put(
        &self,
        stf_info: &StateTransitionInfo<StateRoot, Witness, Da>,
    ) -> anyhow::Result<SchemaBatch> {
        let encoded_stf_info: Vec<u8> = bincode::serialize(stf_info).unwrap();
        let stored_stf_info = StoredStfInfo {
            data: encoded_stf_info,
        };

        self.ledger_db
            .materialize_stf_info(&stored_stf_info, &SlotNumber(stf_info.rollup_height))
    }

    /// Get [`StateTransitionInfo`] for the corresponding rollup height.
    pub(crate) fn get(
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
    use sov_mock_da::{MockBlockHeader, MockDaSpec, MockHash};
    use sov_modules_api::da::Time;
    use sov_rollup_interface::da::{DaProof, RelevantBlobs, RelevantProofs};
    use sov_rollup_interface::zk::StateTransitionWitness;
    use sov_test_utils::storage::SimpleLedgerStorageManager;

    use super::*;
    use crate::StateTransitionInfo;

    #[test]
    fn test_stf_info_in_db() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut storage_manager = SimpleLedgerStorageManager::new(temp_dir.path());
        let ledger_storage = storage_manager.create_ledger_storage();
        let ledger_db = LedgerDb::with_reader(ledger_storage).unwrap();

        let stf_info_manager = StfInfoManager::new(ledger_db);

        // At the begining the db should be empty.
        let fetched_stf_info = stf_info_manager.get(1).unwrap();
        assert!(fetched_stf_info.is_none());

        // Insert astf info two times.
        let header_hash_1 = MockHash([11; 32]);
        assert_stf_in_db(header_hash_1, 1, &mut storage_manager, &stf_info_manager);

        let header_hash_2 = MockHash([22; 32]);
        assert_stf_in_db(header_hash_2, 2, &mut storage_manager, &stf_info_manager);

        // Check if the first stf is still in the db.
        let fetched_stf_info = stf_info_manager.get(1).unwrap();
        assert!(fetched_stf_info.is_some());
    }

    fn assert_stf_in_db(
        header_hash: MockHash,
        rollup_height: u64,
        storage_manager: &mut SimpleLedgerStorageManager,
        stf_info_manager: &StfInfoManager<Vec<u8>, Vec<u8>, MockDaSpec>,
    ) {
        let original_state_transition_info = make_stf_info(header_hash, rollup_height);

        let schema = stf_info_manager
            .put(&original_state_transition_info)
            .unwrap();

        storage_manager.commit(schema);

        let fetched_stf_info = stf_info_manager.get(rollup_height).unwrap().unwrap();

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
}
