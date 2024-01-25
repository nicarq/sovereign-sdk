use sov_blob_storage::BlobStorage;
use sov_chain_state::{ChainState, ChainStateConfig};
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::default_context::DefaultContext;
use sov_modules_api::tx_verifier::RawTx;
use sov_modules_api::{KernelModule, KernelWorkingSet, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;

type C = DefaultContext;
type Da = MockDaSpec;

#[test]
fn empty_test() {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());

    let chain_state = ChainState::<C, Da>::default();
    let initial_slot_height = 1;
    let chain_state_config = ChainStateConfig {
        current_time: Default::default(),
        gas_price_blocks_depth: 10,
        gas_price_maximum_elasticity: 1,
        initial_gas_price: [0, 0],
        minimum_gas_price: [0, 0],
    };
    chain_state
        .genesis_unchecked(
            &chain_state_config,
            &mut KernelWorkingSet::uninitialized(&mut working_set),
        )
        .unwrap();

    let blob_storage = BlobStorage::<C, Da>::default();

    let blobs = blob_storage.take_blobs_for_slot_height(initial_slot_height, &mut working_set);

    assert!(blobs.is_empty());
}

#[test]
fn store_and_retrieve_standard() {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());

    let chain_state = ChainState::<C, Da>::default();
    let chain_state_config = ChainStateConfig {
        current_time: Default::default(),
        gas_price_blocks_depth: 10,
        gas_price_maximum_elasticity: 1,
        initial_gas_price: [0, 0],
        minimum_gas_price: [0, 0],
    };
    chain_state
        .genesis_unchecked(
            &chain_state_config,
            &mut KernelWorkingSet::uninitialized(&mut working_set),
        )
        .unwrap();

    let blob_storage = BlobStorage::<C, Da>::default();

    assert!(blob_storage
        .take_blobs_for_slot_height(1, &mut working_set)
        .is_empty());
    assert!(blob_storage
        .take_blobs_for_slot_height(2, &mut working_set)
        .is_empty());
    assert!(blob_storage
        .take_blobs_for_slot_height(3, &mut working_set)
        .is_empty());
    assert!(blob_storage
        .take_blobs_for_slot_height(4, &mut working_set)
        .is_empty());

    let sender = MockAddress::from([1u8; 32]);

    let mut batches = Vec::new();
    for i in 1..=5 {
        let batch = BatchWithId {
            txs: vec![RawTx {
                data: vec![i * 3 + 1, i * 3 + 2, i * 3 + 3],
            }],
            id: [i; 32],
        };
        batches.push((batch, sender));
    }

    let slot_2_batches = &batches[..3];
    let slot_3_batches = &batches[3..4];
    let slot_4_batches = &batches[4..5];

    blob_storage.store_batches(2, &slot_2_batches.to_vec(), &mut working_set);
    blob_storage.store_batches(3, &slot_3_batches.to_vec(), &mut working_set);
    blob_storage.store_batches(4, &slot_4_batches.to_vec(), &mut working_set);

    assert_eq!(
        slot_2_batches,
        blob_storage.take_blobs_for_slot_height(2, &mut working_set)
    );
    assert!(blob_storage
        .take_blobs_for_slot_height(2, &mut working_set)
        .is_empty());

    assert_eq!(
        slot_3_batches,
        blob_storage
            .take_blobs_for_slot_height(3, &mut working_set)
            .as_slice()
    );
    assert!(blob_storage
        .take_blobs_for_slot_height(3, &mut working_set)
        .is_empty());

    assert_eq!(
        slot_4_batches,
        blob_storage
            .take_blobs_for_slot_height(4, &mut working_set)
            .as_slice()
    );
    assert!(blob_storage
        .take_blobs_for_slot_height(4, &mut working_set)
        .is_empty());
}
