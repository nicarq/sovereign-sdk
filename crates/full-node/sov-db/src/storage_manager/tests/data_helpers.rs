//! To check that [`NativeStorageManager`] creates correct [`DeltaReader`]
//! tests are writing data related to each block,
//! so it can be validated by looking at what data reader can provide.

use rockbound::cache::delta_reader::DeltaReader;
use rockbound::{Schema, SchemaBatch, SchemaKey, SchemaValue, SeekKeyEncoder};
use sov_mock_da::{MockBlockHeader, MockHash};
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::stf::StoredEvent;

use crate::namespaces::UserNamespace;
use crate::schema::namespace::JmtValues;
use crate::schema::tables::{EventByNumber, ModuleAccessoryState};
use crate::schema::types::EventNumber;
use crate::storage_manager::{NativeChangeSet, StfStoragePlaceholder};

// Encoding/Decoding data.

pub fn encode_state_key(height: u64) -> (SchemaKey, jmt::Version) {
    let height_bytes = height.to_be_bytes().to_vec();
    (height_bytes, 0u64)
}

pub fn decode_state_key(raw_key: (SchemaKey, jmt::Version)) -> u64 {
    let (raw_height, version) = raw_key;
    assert_eq!(0, version);
    let bytes = &raw_height[..8];

    u64::from_be_bytes(bytes.try_into().expect("slice with incorrect length"))
}

pub fn decode_state_value(value: Option<SchemaValue>) -> MockHash {
    MockHash::try_from(value.expect("Value must be always set"))
        .expect("Failed to decode mock hash")
}

pub fn decode_state_item(
    item: ((SchemaKey, jmt::Version), Option<SchemaValue>),
) -> (u64, MockHash) {
    let (key, value) = item;
    (decode_state_key(key), decode_state_value(value))
}

fn decode_ledger_item(item: (EventNumber, StoredEvent)) -> (u64, MockHash) {
    let (event_number, stored_event) = item;
    let height = event_number.0;
    assert_eq!(stored_event.key().inner(), stored_event.value().inner());
    let da_hash = MockHash::try_from(stored_event.key().inner().to_vec()).unwrap();
    (height, da_hash)
}

/// Helper for reading data from [`JmtValues<UserNamespace>`].
pub fn get_state_value(
    reader: &DeltaReader,
    key: &(SchemaKey, jmt::Version),
) -> Option<SchemaValue> {
    reader
        .get::<JmtValues<UserNamespace>>(key)
        .unwrap()
        .unwrap()
}

pub fn produce_single_entry_native_changes(
    key: &(SchemaKey, jmt::Version),
    value: &Option<SchemaValue>,
) -> NativeChangeSet {
    let mut stf_changes = NativeChangeSet::default();
    stf_changes
        .state_change_set
        .put::<JmtValues<UserNamespace>>(key, value)
        .unwrap();
    stf_changes
}

// Materializing changes.

/// Build [`NativeChangeSet`] that contains data related to given [`MockBlockHeader`].
/// What it writes:
///  - [`JmtValues<UserNamespace>`]: block_height => block_hash.
///  - [`ModuleAccessoryState`]: block_height => block_hash.
pub fn materialize_stf_changes(da_header: &MockBlockHeader) -> NativeChangeSet {
    let mut state_change_set = SchemaBatch::default();
    let mut accessory_change_set = SchemaBatch::default();

    let key = encode_state_key(da_header.height());

    let hash_bytes = da_header.hash().0.to_vec();
    let value = Some(hash_bytes);

    state_change_set
        .put::<JmtValues<UserNamespace>>(&key, &value)
        .unwrap();

    accessory_change_set
        .put::<ModuleAccessoryState>(&key, &value)
        .unwrap();

    NativeChangeSet {
        state_change_set,
        accessory_change_set,
    }
}

/// Using [`MockBlockHeader::height`] as a key and [`MockHash`] of header as value.
/// What it writes:
///  - [`EventByNumber`] => `block_height` => Event {key == block_hash, value == block_hash}
/// So it can be validated by traversing data from DB.
pub fn materialize_ledger_changes(da_header: &MockBlockHeader) -> SchemaBatch {
    let mut change_set = SchemaBatch::default();
    let key = &EventNumber(da_header.height());
    let value = StoredEvent::new(&da_header.hash().0, &da_header.hash().0);

    change_set.put::<EventByNumber>(key, &value).unwrap();

    change_set
}

// Verifying

pub fn verify_reader<S: Schema, Sk: SeekKeyEncoder<S>, F>(
    reader: &DeltaReader,
    range: std::ops::Range<Sk>,
    expected_values: &[(u64, MockHash)],
    mapper_fn: F,
) where
    F: Fn((S::Key, S::Value)) -> (u64, MockHash),
{
    let actual_values: Vec<(u64, MockHash)> = reader
        .collect_in_range::<S, Sk>(range)
        .unwrap()
        .into_iter()
        .map(mapper_fn)
        .collect();
    assert_eq!(expected_values, &actual_values);
}

/// Check that only expected heights and block hashes are available to given NativeStorage.
pub fn verify_stf_storage(
    stf_storage: &StfStoragePlaceholder,
    expected_values: &[(u64, MockHash)],
) {
    // We take the whole range to check if there's some junk data.
    let range = encode_state_key(0)..encode_state_key(u64::MAX);

    verify_reader::<JmtValues<UserNamespace>, _, _>(
        &stf_storage.state_reader,
        range.clone(),
        expected_values,
        decode_state_item,
    );

    verify_reader::<ModuleAccessoryState, _, _>(
        &stf_storage.accessory_reader,
        range,
        expected_values,
        decode_state_item,
    );
}

pub fn verify_ledger_storage(reader: &DeltaReader, expected_values: &[(u64, MockHash)]) {
    let range = EventNumber(0)..EventNumber(u64::MAX);
    verify_reader::<EventByNumber, _, _>(reader, range, expected_values, decode_ledger_item);
}

pub fn get_expected_chain_values(processed_chain: &[MockBlockHeader]) -> Vec<(u64, MockHash)> {
    processed_chain
        .iter()
        .map(|b| (b.height(), b.hash()))
        .collect()
}
