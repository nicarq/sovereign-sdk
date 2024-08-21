mod arbitrary;
mod data_helpers;

use std::collections::VecDeque;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use proptest::prelude::*;
use rockbound::cache::delta_reader::DeltaReader;
use rockbound::SchemaBatch;
use sov_mock_da::{MockBlockHeader, MockHash};
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use tokio::time::Instant;

use crate::accessory_db::AccessoryDb;
use crate::state_db::StateDb;
use crate::storage_manager::tests::arbitrary::{get_block_hash, ForkDescription, ForkMap};
use crate::storage_manager::tests::data_helpers::{
    encode_height, encode_height_as_key_hash, encode_state_key, get_expected_chain_values,
    get_state_value, materialize_ledger_changes, materialize_stf_changes,
    produce_single_entry_native_changes, verify_ledger_storage, verify_stf_storage, VERSION,
};
use crate::storage_manager::{NativeChangeSet, NativeStorageManager};
use crate::test_utils::TestNativeStorage;

type Da = sov_mock_da::MockDaSpec;

type S = TestNativeStorage;

// Checking the typical lifecycle of the storage in linear progression of the chain,
// meaning no forks happen, and DA height progresses incrementally by 1.
// At each height:
// 1. Bootstrap storage is created and validated.
// 2. Storage for this block is created.
// 3. Changes for this block are materialized and saved.
// 4. If finalization needs to happen for some block, finalization happens.
fn linear_progression(to_height: u64, finality: u64) {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();

    let mut chain = Vec::with_capacity(to_height as usize);
    // Starting from height 1, as 0 is genesis
    for height in 1..=to_height {
        let (bootstrap_stf_storage, bootstrap_ledger_storage) =
            storage_manager.create_bootstrap_state().unwrap();

        let finalized_index = chain.len().saturating_sub(finality as usize);
        let expected_finalized_values = get_expected_chain_values(&chain[..finalized_index]);
        verify_stf_storage(&bootstrap_stf_storage, &expected_finalized_values[..]);
        verify_ledger_storage(&bootstrap_ledger_storage, &expected_finalized_values[..]);

        let da_header = MockBlockHeader::from_height(height);
        let (stf_storage, ledger_storage) = storage_manager.create_state_for(&da_header).unwrap();
        let expected_values = get_expected_chain_values(&chain[..]);
        verify_stf_storage(&stf_storage, &expected_values[..]);
        verify_ledger_storage(&ledger_storage, &expected_values[..]);

        let stf_changes = materialize_stf_changes(&da_header);
        let ledger_changes = materialize_ledger_changes(&da_header);
        storage_manager
            .save_change_set(&da_header, stf_changes, ledger_changes)
            .unwrap();
        assert_eq!(
            storage_manager.snapshots.len(),
            storage_manager.blocks_to_parent.len()
        );
        // This block is considered to be "processed", saving it.
        chain.push(da_header.clone());
        let (stf_storage, ledger_storage) = storage_manager.create_state_after(&da_header).unwrap();
        let expected_values = get_expected_chain_values(&chain[..]);
        verify_stf_storage(&stf_storage, &expected_values[..]);
        verify_ledger_storage(&ledger_storage, &expected_values[..]);

        if let Some(final_height) = height.checked_sub(finality) {
            if final_height > 0 {
                let final_header = MockBlockHeader::from_height(final_height);
                storage_manager.finalize(&final_header).unwrap();
            }
        }
    }

    let should_be_empty = finality == 0;
    assert_eq!(should_be_empty, storage_manager.is_empty());
}

enum ExplorationMode {
    // Breath first.
    Bfs,
    // Depth first.
    Dfs,
}

impl ExplorationMode {
    fn get_next_hash(&self, queue: &mut VecDeque<MockHash>) -> Option<MockHash> {
        match self {
            ExplorationMode::Bfs => queue.pop_front(),
            ExplorationMode::Dfs => queue.pop_back(),
        }
    }
}

// Create and save changes from all forks, iterating by height
fn test_exploration(fork: ForkDescription, exploration_mode: ExplorationMode) {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();

    let fork_map = ForkMap::from(fork);
    let start = fork_map.get_start().expect("Empty chain-map");
    let mut next_blocks = VecDeque::new();
    next_blocks.push_back(start);

    let mut highest_block = MockBlockHeader::from_height(0);

    while let Some(block_hash) = exploration_mode.get_next_hash(&mut next_blocks) {
        for child in fork_map.get_child_hashes(&block_hash) {
            next_blocks.push_back(child);
        }

        let da_header = fork_map.get_block_header(&block_hash).unwrap();
        if da_header.height() > highest_block.height() {
            highest_block = da_header.clone();
        }

        let (stf_storage, ledger_storage) = storage_manager
            .create_state_for(da_header)
            .expect("Creating storage failed");

        let this_chain = fork_map.get_chain_up_to(da_header.clone());
        let expected_values = get_expected_chain_values(&this_chain[..this_chain.len() - 1]);
        verify_stf_storage(&stf_storage, &expected_values[..]);
        verify_ledger_storage(&ledger_storage, &expected_values[..]);

        let stf_changes = materialize_stf_changes(da_header);
        let ledger_changes = materialize_ledger_changes(da_header);

        storage_manager
            .save_change_set(da_header, stf_changes, ledger_changes)
            .expect("Saving change set has failed");
        assert_eq!(
            storage_manager.snapshots.len(),
            storage_manager.blocks_to_parent.len()
        );

        let (stf_storage, ledger_storage) = storage_manager.create_state_after(da_header).unwrap();
        assert_eq!(
            storage_manager.snapshots.len(),
            storage_manager.blocks_to_parent.len()
        );
        let expected_values = get_expected_chain_values(&this_chain[..this_chain.len()]);
        verify_stf_storage(&stf_storage, &expected_values[..]);
        verify_ledger_storage(&ledger_storage, &expected_values[..]);
    }
    // Finalizing longest chain
    let longest_chain = fork_map.get_chain_up_to(highest_block);
    for block_header in longest_chain {
        storage_manager.finalize(&block_header).unwrap();
    }
    // State manager should be empty after the longest chain is finalized.
    assert!(storage_manager.is_empty());
}

#[test_log::test]
fn minimal_fork_bfs() {
    let fork = ForkDescription {
        start_height: 1,
        length: 3,
        child_forks: vec![
            ForkDescription {
                start_height: 1,
                length: 5,
                child_forks: Vec::new(),
            },
            ForkDescription {
                start_height: 1,
                length: 5,
                child_forks: Vec::new(),
            },
        ],
    };
    test_exploration(fork.clone(), ExplorationMode::Bfs);
    test_exploration(fork, ExplorationMode::Dfs);
}

#[test]
fn calls_on_empty() {
    let tmpdir = tempfile::tempdir().unwrap();

    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();
    assert!(storage_manager.is_empty());
    #[cfg(debug_assertions)]
    storage_manager.validate_internal_consistency();

    let da_header = MockBlockHeader::from_height(1);

    let save_result = storage_manager.save_change_set(
        &da_header,
        NativeChangeSet::default(),
        SchemaBatch::default(),
    );
    assert!(save_result.is_err());
    #[cfg(debug_assertions)]
    storage_manager.validate_internal_consistency();
    let expected_msg = format!(
        "Attempt to save changeset for unknown block header {}",
        da_header.display()
    );
    assert_eq!(expected_msg, save_result.unwrap_err().to_string());

    let finalize_result = storage_manager.finalize(&da_header);
    assert!(finalize_result.is_err());
    #[cfg(debug_assertions)]
    storage_manager.validate_internal_consistency();
    let expected_msg = format!(
        "No changes has been previously saved for block header prev_hash={} next_hash={}",
        da_header.prev_hash, da_header.hash,
    );
    assert_eq!(expected_msg, finalize_result.unwrap_err().to_string());
    assert!(storage_manager.is_empty());
}

#[test_log::test]
fn linear_progression_instant_finality() {
    linear_progression(5, 0);
}

#[test_log::test]
fn linear_progression_non_instant_finality() {
    linear_progression(5, 1);
    linear_progression(5, 4);
    linear_progression(5, 5);
    linear_progression(5, 6);
    linear_progression(5, 10);
}

proptest! {
    #[test]
    fn proptest_iteration(fork in any::<ForkDescription>()) {
        test_exploration(fork.clone(), ExplorationMode::Bfs);
        test_exploration(fork, ExplorationMode::Dfs);
    }
}

#[test]
fn double_create_storage() {
    // Checks that calling creates storage multiple times for the same block works without errors
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();

    let da_header = MockBlockHeader::from_height(1);
    // Bootstrap storage
    let _ = storage_manager.create_bootstrap_state().unwrap();
    let _ = storage_manager.create_bootstrap_state().unwrap();

    // Normal storage
    let _ = storage_manager.create_state_for(&da_header).unwrap();
    let _ = storage_manager.create_state_for(&da_header).unwrap();

    let stf_changes = materialize_stf_changes(&da_header);
    let ledger_changes = materialize_ledger_changes(&da_header);
    storage_manager
        .save_change_set(&da_header, stf_changes, ledger_changes)
        .unwrap();

    // After block
    let _ = storage_manager.create_state_after(&da_header).unwrap();
    let _ = storage_manager.create_state_after(&da_header).unwrap();
}

fn attempt_to_save_unknown_block(
    storage_manager: &mut NativeStorageManager<Da, S>,
    da_header: &MockBlockHeader,
) {
    let result = storage_manager.save_change_set(
        da_header,
        NativeChangeSet::default(),
        SchemaBatch::default(),
    );
    assert!(result.is_err());
    #[cfg(debug_assertions)]
    storage_manager.validate_internal_consistency();
    let expected_error_message = format!(
        "Attempt to save changeset for unknown block header {}",
        da_header.display()
    );
    assert_eq!(expected_error_message, result.unwrap_err().to_string());
}

#[test]
fn unknown_block_cannot_be_saved() {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();

    // On an empty map
    let da_header_1 = MockBlockHeader::from_height(1);
    attempt_to_save_unknown_block(&mut storage_manager, &da_header_1);
    let _ = storage_manager.create_state_for(&da_header_1).unwrap();
    let stf_changes = materialize_stf_changes(&da_header_1);
    let ledger_changes = materialize_ledger_changes(&da_header_1);
    storage_manager
        .save_change_set(&da_header_1, stf_changes, ledger_changes)
        .unwrap();

    // On something
    let da_header_2 = MockBlockHeader::from_height(2);
    attempt_to_save_unknown_block(&mut storage_manager, &da_header_2);
}

#[test]
fn double_save_changes() {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();
    let da_header = MockBlockHeader::from_height(1);
    let _ = storage_manager.create_state_for(&da_header).unwrap();
    // the first save everything is good.
    storage_manager
        .save_change_set(&da_header, NativeChangeSet::default(), SchemaBatch::new())
        .unwrap();

    // the second save should fail, as it indicates a bug.
    let result = storage_manager.save_change_set(
        &da_header,
        NativeChangeSet::default(),
        SchemaBatch::default(),
    );
    assert!(result.is_err());
    #[cfg(debug_assertions)]
    storage_manager.validate_internal_consistency();
    let expected_message = format!(
        "Attempt to save changes for the same block {} twice. Probably a bug.",
        da_header.display()
    );
    assert_eq!(expected_message, result.unwrap_err().to_string());
}

#[test]
fn create_state_after_not_saved_block() {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();
    let da_header = MockBlockHeader::from_height(1);

    let _ = storage_manager.create_state_for(&da_header).unwrap();

    // It should throw the error as changes for this block is not available.
    let result = storage_manager.create_state_after(&da_header);
    assert!(result.is_err());
    #[cfg(debug_assertions)]
    storage_manager.validate_internal_consistency();
    let expected_message = format!("There is no snapshot available for the block {}. Use `create_bootstrap_storage` for getting storage from finalized data.", da_header.display());
    assert_eq!(expected_message, result.unwrap_err().to_string());
    #[cfg(debug_assertions)]
    storage_manager.validate_internal_consistency();

    storage_manager
        .save_change_set(&da_header, NativeChangeSet::default(), SchemaBatch::new())
        .unwrap();

    // Now storage "after" the block can be created, as there's a snapshot for this block.
    let _ = storage_manager.create_state_after(&da_header).unwrap();
    storage_manager.finalize(&da_header).unwrap();
    assert!(storage_manager.is_empty());
    // But not after it has been finalized.
    let result = storage_manager.create_state_after(&da_header);
    assert!(result.is_err());
    assert_eq!(expected_message, result.unwrap_err().to_string());
}

#[test]
fn finalize_only_last_block() {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();
    let to_height = 5;
    for height in 1..=to_height {
        let da_header = MockBlockHeader::from_height(height);
        let _ = storage_manager.create_state_for(&da_header).unwrap();
        let native_changes = materialize_stf_changes(&da_header);
        let ledger_changes = materialize_ledger_changes(&da_header);
        storage_manager
            .save_change_set(&da_header, native_changes, ledger_changes)
            .unwrap();
    }
    let da_header = MockBlockHeader::from_height(to_height);
    storage_manager.finalize(&da_header).unwrap();
    assert!(&storage_manager.is_empty());
}

#[test]
fn parallel_forks_reading_while_finalization_is_happening() {
    // 1    2    ..   n-1   n
    //                 / -> E
    // A -> B -> .. -> C -> D
    //                 \ -> F
    //                      ..
    //                   .. X
    // E, H, G, etc. are moved to a separate thread.
    // They read data from each snapshot all the time,
    // checking that data from each for is present.

    // Validation:
    // First test measures how much time on average it takes
    // to do such validation single threaded without any concurrent readings
    // Then it starts X threads for each "fork" to do the same validation.
    // Each thread does 2 iterations of reading:
    // just concurrent reading and then concurrent reading while blocks are finalized.
    // Then test checks
    // that avg time each thread spent on these is not more than 3 times a single reading.

    // this is X.
    // So the total value of concurrent threads will be 8,
    // which is a comfortable choice for many machines.
    let sub_forks_count = 7;
    // this is n
    // Enough blocks will be written on disk during the finalization phase.
    let main_fork_len = 30;
    let fork_description = ForkDescription {
        start_height: 1,
        length: main_fork_len,
        child_forks: vec![
            ForkDescription {
                start_height: (main_fork_len - 1) as u64,
                length: 1,
                child_forks: Vec::new(),
            };
            sub_forks_count
        ],
    };

    let fork_map = ForkMap::from(fork_description);
    assert_eq!(
        sub_forks_count + main_fork_len as usize,
        fork_map.blocks_count()
    );

    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();

    // fill storage manager
    let start = fork_map.get_start().expect("Empty chain-map");
    let mut next_blocks = VecDeque::new();
    next_blocks.push_back(start);
    while let Some(block_hash) = next_blocks.pop_front() {
        for child in fork_map.get_child_hashes(&block_hash) {
            next_blocks.push_back(child);
        }
        let da_header = fork_map.get_block_header(&block_hash).unwrap();
        let _ = storage_manager
            .create_state_for(da_header)
            .expect("Creating storage failed");
        let native_changes = materialize_stf_changes(da_header);
        let ledger_changes = materialize_ledger_changes(da_header);
        storage_manager
            .save_change_set(da_header, native_changes, ledger_changes)
            .expect("Saving change set has failed");
    }

    let mut prepare_for_reading = |fork_id: u64| {
        let block_hash = get_block_hash(fork_id, main_fork_len as u64);
        let block_header = fork_map.get_block_header(&block_hash).unwrap();
        let this_chain = fork_map.get_chain_up_to(block_header.clone());
        let expected_values = get_expected_chain_values(&this_chain[..this_chain.len()]);
        let (stf_storage, ledger_storage) = storage_manager
            .create_state_after(block_header)
            .expect("Creating storage failed");

        (stf_storage, ledger_storage, expected_values)
    };

    let reading_count = 1000;

    let record_reading =
        move |stf: &S, ledger: &DeltaReader, expected: &[(u64, MockHash)]| -> Duration {
            let mut spent_reading = Duration::default();
            for _ in 0..reading_count {
                let start_validation = Instant::now();
                verify_stf_storage(stf, expected);
                verify_ledger_storage(ledger, expected);
                spent_reading += start_validation.elapsed();
            }
            spent_reading / reading_count
        };

    // Record how much it takes to do a round of reading from the main fork without any concurrency.
    let average_reading_time_single_access = {
        let (stf_storage, ledger_storage, expected_values) = prepare_for_reading(1);
        record_reading(&stf_storage, &ledger_storage, &expected_values[..])
    };

    let avg_reading_time_threshold = average_reading_time_single_access.checked_mul(3).unwrap();

    let total_forks = sub_forks_count + 1; // main fork
    let barrier = Arc::new(Barrier::new(total_forks));

    let mut handles = vec![];
    // Starting fork readers
    for fork_id in 1..total_forks {
        let (stf_storage, ledger_storage, expected_values) = prepare_for_reading(fork_id as u64);

        let barrier = Arc::clone(&barrier);

        // Each fork counts how many reads it completed.
        handles.push(std::thread::spawn(move || -> (Duration, Duration) {
            // First, we record how much time each thread took reading concurrently;
            let spent_reading_concurrently =
                record_reading(&stf_storage, &ledger_storage, &expected_values[..]);

            // Then we wait for finalization to start
            barrier.wait();

            let spent_reading_during_finalization =
                record_reading(&stf_storage, &ledger_storage, &expected_values[..]);
            (
                spent_reading_concurrently,
                spent_reading_during_finalization,
            )
        }));
    }

    barrier.wait();
    let mut finalization_duration = Duration::default();
    for height in 1..=main_fork_len {
        let start = Instant::now();
        let block_hash = get_block_hash(1, height as u64);
        let block_header = fork_map.get_block_header(&block_hash).unwrap();
        storage_manager.finalize(block_header).unwrap();
        finalization_duration += start.elapsed();
    }
    for handle in handles {
        let (spent_reading_concurrently, spent_reading_finalization) =
            handle.join().expect("Thread panicked");
        assert!(
            spent_reading_concurrently < avg_reading_time_threshold,
            "Concurrent reading {:?} is worse than max allowed {:?}",
            spent_reading_concurrently,
            avg_reading_time_threshold
        );
        assert!(
            spent_reading_finalization < avg_reading_time_threshold,
            "Concurrent reading during finalization {:?} is worse than max allowed {:?}",
            spent_reading_finalization,
            avg_reading_time_threshold
        );
    }
    assert!(storage_manager.is_empty());
}

#[test]
fn check_snapshots_ordering() {
    // Other tests check that each fork has only correct snapshots,
    // but they don't check if those snapshots are in the right order.
    // This test validates that.

    // It is similar to [`linear_progression`], but it writes different data.
    // block 1 writes its own hash to all keys from 1 to max height.
    // block 2 writes its own hash to all keys from 1 to max_height - 1.
    // ...
    // the last block only writes its own hash to key=1.
    // If snapshots are out of order, expected values will be "shadowed".

    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();

    let to_height = 5;
    let mut expected_values = Vec::with_capacity(to_height as usize);

    for height in 1..=to_height {
        let da_header = MockBlockHeader::from_height(height);
        let _ = storage_manager.create_state_for(&da_header).unwrap();

        let hash_bytes = da_header.hash().0.to_vec();
        let write_to = to_height
            .checked_sub(height)
            .unwrap()
            .checked_add(1)
            .unwrap();
        expected_values.push((write_to, da_header.hash()));

        let state_values_to_materialize = (1..=write_to).map(|k| {
            let key_as_hash = encode_height_as_key_hash(k);
            (key_as_hash, &hash_bytes)
        });
        let state_change_set =
            StateDb::materialize_preimages([], state_values_to_materialize).unwrap();

        let accessory_values_to_materialize = (1..=write_to).map(|k| {
            let key = encode_height(k).to_vec();
            (key, Some(hash_bytes.clone()))
        });
        let accessory_change_set =
            AccessoryDb::materialize_values(accessory_values_to_materialize, VERSION).unwrap();

        let stf_change_set = NativeChangeSet {
            state_change_set,
            accessory_change_set,
        };

        storage_manager
            .save_change_set(&da_header, stf_change_set, SchemaBatch::default())
            .unwrap();
    }
    // Validation.
    // Default ordering is from low to high, but we put expected values in reverse.
    expected_values.reverse();
    let da_header = MockBlockHeader::from_height(to_height);
    let (stf_storage, _) = storage_manager.create_state_after(&da_header).unwrap();

    verify_stf_storage(&stf_storage, &expected_values);
}

#[test]
fn several_jumping_forks() {
    // At each height there happens x forks.
    // They all create storage for themselves.
    // Then they all save some changes.
    // Then 1 is finalized after x blocks.
    // The purpose of this is to check that at a given height,
    // several storages can be created without saving all the data.
    let to_height = 10;
    let finality = 3;
    let forks_number = 5;
    let main_fork_id = 1;

    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();

    let mut fork_headers = Vec::with_capacity(forks_number as usize);
    for height in 1..=to_height {
        fork_headers.clear();
        for fork_id in 1..=forks_number {
            let prev_hash = get_block_hash(fork_id, height - 1);
            let hash = get_block_hash(fork_id, height);
            fork_headers.push(MockBlockHeader {
                prev_hash,
                hash,
                height,
                ..Default::default()
            });
        }
        // Create storage for each fork.
        for da_header in &fork_headers {
            let _ = storage_manager.create_state_for(da_header).unwrap();
        }
        // Save storage for each fork.
        for da_header in &fork_headers {
            storage_manager
                .save_change_set(
                    da_header,
                    NativeChangeSet::default(),
                    SchemaBatch::default(),
                )
                .unwrap();
        }

        if let Some(final_height) = height.checked_sub(finality) {
            if final_height > 0 {
                let prev_hash = get_block_hash(main_fork_id, final_height - 1);
                let hash = get_block_hash(main_fork_id, final_height);
                let final_header = MockBlockHeader {
                    prev_hash,
                    hash,
                    height: final_height,
                    ..Default::default()
                };
                storage_manager.finalize(&final_header).unwrap();
            }
        }
    }
}

// 2 helper functions for the following tests.

// Here is chain schema:
//  A -> B
//   \-> C
// Returns A, B, C
fn get_abc_blocks() -> (MockBlockHeader, MockBlockHeader, MockBlockHeader) {
    let block_a = MockBlockHeader::from_height(1);
    // Changes are in rocksdb now, creating readers
    let block_b = MockBlockHeader {
        prev_hash: block_a.hash(),
        hash: get_block_hash(1, 2),
        height: 2,
        ..Default::default()
    };
    let block_c = MockBlockHeader {
        prev_hash: block_a.hash(),
        hash: get_block_hash(2, 2),
        height: 2,
        ..Default::default()
    };
    (block_a, block_b, block_c)
}

#[test]
fn removed_fork_data_view() {
    // "Orphaned fork" is a fork appears when finalization happens on different fork past the start height of this fork.
    // Meaning that this fork should be discarded completely, because data from the sibling fork was finalized.
    // This test documents behavior that observed from this orphaned fork.
    // This might be useful to know if there's a long-running task that relies on data from a fork that has been orphaned.
    // Test details.
    // Here is the chain schema:
    //  A -> B
    //   \-> C
    // Block A has key=1 value=1.
    // This block is finalized.
    // Blocks B and C are created after block A has been finalized. They both observer key=1 value=1
    //
    // Block C observes:
    // - key 1 value "swapped" from 1 to 2, because it was looking at rocksdb.

    let key = encode_state_key(1);
    let value_1 = Some(vec![1u8; 32]);
    let value_2 = Some(vec![2u8; 32]);

    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();

    // Just to put data into rocksdb
    let (block_a, block_b, block_c) = get_abc_blocks();

    // Operations in Block A
    let _ = storage_manager.create_state_for(&block_a).unwrap();
    let stf_changes = produce_single_entry_native_changes(&key, &value_1);
    storage_manager
        .save_change_set(&block_a, stf_changes, SchemaBatch::default())
        .unwrap();
    storage_manager.finalize(&block_a).unwrap();
    assert!(storage_manager.is_empty());

    // Changes are in rocksdb now, creating readers
    let (stf_reader_b, _) = storage_manager.create_state_for(&block_b).unwrap();
    let (stf_reader_c, _) = storage_manager.create_state_for(&block_c).unwrap();

    let value_at_b = get_state_value(&stf_reader_b.state, &key);
    assert_eq!(value_1, value_at_b);
    let value_at_c = get_state_value(&stf_reader_c.state, &key);
    assert_eq!(value_1, value_at_c);

    // Saving block B, data is correct
    let stf_changes = produce_single_entry_native_changes(&key, &value_2);
    storage_manager
        .save_change_set(&block_b, stf_changes, SchemaBatch::default())
        .unwrap();
    assert!(!storage_manager.is_empty());
    let value_at_b = get_state_value(&stf_reader_b.state, &key);
    assert_eq!(value_1, value_at_b);
    let value_at_c = get_state_value(&stf_reader_c.state, &key);
    assert_eq!(value_1, value_at_c);

    // Finalizing block B
    storage_manager.finalize(&block_b).unwrap();
    assert!(storage_manager.snapshots.is_empty());

    let value_at_c = get_state_value(&stf_reader_c.state, &key);
    // Now it suddenly has `value_2`, instead of `value_1` that has been observed previously.
    assert_eq!(value_2, value_at_c);
}

#[test]
fn fork_keeps_reference_to_snapshot_after_finalization() {
    // This test is similar to `removed_fork_data_view`,
    // But it demonstrates that change happens only to data that has been in rocksdb before the block is created.
    // Data that has been in the snapshot when a block has been created remains the same for the fork.

    let key = encode_state_key(1);
    let value_1 = Some(vec![1u8; 32]);
    let value_2 = Some(vec![2u8; 32]);
    // Just to put data into rocksdb
    let (block_a, block_b, block_c) = get_abc_blocks();

    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = NativeStorageManager::<Da, S>::new(tmpdir.path()).unwrap();

    let _ = storage_manager.create_state_for(&block_a).unwrap();
    let stf_changes = produce_single_entry_native_changes(&key, &value_1);
    storage_manager
        .save_change_set(&block_a, stf_changes, SchemaBatch::default())
        .unwrap();

    let (stf_reader_b, _) = storage_manager.create_state_for(&block_b).unwrap();
    let (stf_reader_c, _) = storage_manager.create_state_for(&block_c).unwrap();

    let value_at_b = get_state_value(&stf_reader_b.state, &key);
    assert_eq!(value_1, value_at_b);
    let value_at_c = get_state_value(&stf_reader_c.state, &key);
    assert_eq!(value_1, value_at_c);

    let stf_changes = produce_single_entry_native_changes(&key, &value_2);
    storage_manager
        .save_change_set(&block_b, stf_changes, SchemaBatch::default())
        .unwrap();

    storage_manager.finalize(&block_a).unwrap();
    storage_manager.finalize(&block_b).unwrap();

    // reader_c still observes data from block A,
    // even though it has been overwritten in rocksdb by block B
    let value_at_c = get_state_value(&stf_reader_c.state, &key);
    assert_eq!(value_1, value_at_c);
}
