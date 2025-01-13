//! To check that [`NativeStorageManager`] creates correct [`DeltaReader`]
//! tests are writing data related to each block,
//! so it can be validated by looking at what data reader can provide.

use jmt::storage::HasPreimage;
use jmt::KeyHash;
use rockbound::cache::delta_reader::DeltaReader;
use rockbound::{SchemaBatch, SchemaKey, SchemaValue};
use sov_mock_da::{MockBlockHeader, MockHash};
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::stf::StoredEvent;

use crate::accessory_db::AccessoryDb;
use crate::namespaces::{KernelNamespace, UserNamespace};
use crate::schema::tables::EventByNumber;
use crate::schema::types::EventNumber;
use crate::state_db::StateDb;
use crate::storage_manager::tests::TestNativeStorage;
use crate::storage_manager::NativeChangeSet;
use crate::test_utils::build_node_batch;
// Encoding/Decoding data.

type H = sha2::Sha256;
pub type N = UserNamespace;
pub const VERSION: jmt::Version = 0;

pub fn encode_state_key(height: u64) -> (SchemaKey, jmt::Version) {
    let height_bytes = height.to_be_bytes().to_vec();
    (height_bytes, VERSION)
}

pub fn encode_height(height: u64) -> [u8; 32] {
    let mut array: [u8; 32] = [0; 32];
    let bytes = height.to_be_bytes();
    array[(32 - bytes.len())..].copy_from_slice(&bytes);
    array
}

pub fn encode_height_as_key_hash(height: u64) -> KeyHash {
    KeyHash(encode_height(height))
}

fn decode_ledger_item(item: (EventNumber, StoredEvent)) -> (u64, MockHash) {
    let (event_number, stored_event) = item;
    let height = event_number.0;
    assert_eq!(stored_event.key().inner(), stored_event.value().inner());
    let da_hash = MockHash::try_from(stored_event.key().inner().to_vec()).unwrap();
    (height, da_hash)
}

/// Helper for reading data from [`JmtValues<N>`].
pub fn get_state_value(state_db: &StateDb, key: &(SchemaKey, jmt::Version)) -> Option<SchemaValue> {
    let (key, version) = key;
    state_db
        .get_value_option_by_key::<N>(SlotNumber::new_dangerous(*version), key)
        .unwrap()
}

pub fn produce_single_entry_native_changes(
    state_db: &StateDb,
    key: &(SchemaKey, jmt::Version),
    value: &Option<SchemaValue>,
) -> NativeChangeSet {
    let key_hash = KeyHash::with::<H>(&key.0);
    let materialized_preimages =
        StateDb::materialize_preimages(vec![(key_hash, &key.0)], vec![(key_hash, &key.0)]).unwrap();

    let jmt_handler_user = state_db.get_jmt_handler::<UserNamespace>();
    let jmt_handler_kernel = state_db.get_jmt_handler::<KernelNamespace>();

    let node_batch_user =
        build_node_batch::<_, H>(&jmt_handler_user, key.1, vec![(key_hash, value.clone())]);
    let node_batch_kernel =
        build_node_batch::<_, H>(&jmt_handler_kernel, key.1, vec![(key_hash, value.clone())]);

    let state_change_set = state_db
        .materialize_node_batches(
            &node_batch_kernel,
            &node_batch_user,
            Some(materialized_preimages),
        )
        .unwrap();

    NativeChangeSet {
        state_change_set,
        ..Default::default()
    }
}

// Materializing changes.

/// Build [`NativeChangeSet`] that contains data related to given [`MockBlockHeader`].
/// What it writes:
///  - [`KeyHashToKey<N>`]: block_height => block_hash.
///  - [`ModuleAccessoryState`]: block_height => block_hash.
pub fn materialize_stf_changes(da_header: &MockBlockHeader) -> NativeChangeSet {
    // State
    let key_as_hash = encode_height_as_key_hash(da_header.height());
    let hash_bytes = da_header.hash().0.to_vec();

    let item = (key_as_hash, &hash_bytes);
    let state_change_set = StateDb::materialize_preimages([], [item]).unwrap();

    // Accessory
    let accessory_key = encode_height(da_header.height()).to_vec();
    let accessory_change_set = AccessoryDb::materialize_values(
        [(accessory_key, Some(hash_bytes))],
        SlotNumber::new_dangerous(VERSION),
    )
    .unwrap();
    NativeChangeSet {
        state_change_set,
        accessory_change_set,
    }
}

/// Using [`MockBlockHeader::height`] as a key and [`MockHash`] of header as value.
///
/// What it writes:
///  - [`EventByNumber`] => `block_height` => Event {key == block_hash, value == block_hash}
///
/// So it can be validated by traversing data from DB.
pub fn materialize_ledger_changes(da_header: &MockBlockHeader) -> SchemaBatch {
    let mut change_set = SchemaBatch::default();
    let key = &EventNumber(da_header.height());
    let value = StoredEvent::new(&da_header.hash().0, &da_header.hash().0);

    change_set.put::<EventByNumber>(key, &value).unwrap();

    change_set
}

// Verifying
pub fn verify_state_db(state_db: &StateDb, expected_values: &[(u64, MockHash)]) {
    // We cannot check that extra data hasn't been written,
    // because StateDb does not expose range API, but this is highly unlikely that some data is copied.
    // And it will be better tested by integration tests with business logic.
    // This test at least test presence of
    let jmt_handler = state_db.get_jmt_handler::<N>();
    for (expected_height, expected_hash) in expected_values {
        let key_hash = encode_height_as_key_hash(*expected_height);
        let pre_image = jmt_handler.preimage(key_hash).unwrap().unwrap();
        assert_eq!(expected_hash.0.to_vec(), pre_image);
    }
}

pub fn verify_accessory_db(accessory_db: &AccessoryDb, expected_values: &[(u64, MockHash)]) {
    for (expected_height, expected_hash) in expected_values {
        let key = encode_height(*expected_height).to_vec();
        let actual_value = accessory_db
            .get_value_option(&key, SlotNumber::new_dangerous(VERSION))
            .unwrap()
            .expect("Missing value in AccessoryDb");
        assert_eq!(expected_hash.0.to_vec(), actual_value);
    }
}

/// Check that only expected heights and block hashes are available to given NativeStorage.
pub fn verify_stf_storage(stf_storage: &TestNativeStorage, expected_values: &[(u64, MockHash)]) {
    verify_state_db(&stf_storage.state, expected_values);
    verify_accessory_db(&stf_storage.accessory_db, expected_values);
}

pub fn verify_ledger_storage(reader: &DeltaReader, expected_values: &[(u64, MockHash)]) {
    let range = EventNumber(0)..EventNumber(u64::MAX);

    let actual_values: Vec<(u64, MockHash)> = reader
        .collect_in_range::<EventByNumber, _>(range)
        .unwrap()
        .into_iter()
        .map(decode_ledger_item)
        .collect();
    assert_eq!(expected_values, &actual_values);
}

pub fn get_expected_chain_values(processed_chain: &[MockBlockHeader]) -> Vec<(u64, MockHash)> {
    processed_chain
        .iter()
        .map(|b| (b.height(), b.hash()))
        .collect()
}
