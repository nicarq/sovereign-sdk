use std::str::FromStr;

use anyhow::{Context, Error};
use borsh::{BorshDeserialize, BorshSerialize};
use sov_modules_core::AddressBech32;
use sov_rollup_interface::rpc::{
    EventIdentifier, EventResponse, LedgerRpcProvider, PaginatedEventResponse,
};
use sov_rollup_interface::stf::EventKey;

use crate::ledger_db::LedgerDB;
use crate::schema::tables::{EventByKey, EventByModuleAddress};
use crate::schema::types::{EventNumber, ModuleAddress, TxNumber};

fn event_match_helper(
    scanned_key: &EventKey,
    provided_key: &str,
    scanned_address: &[u8],
    provided_address: &Option<Vec<u8>>,
    scanned_txn_num: u64,
    provided_txn_range: Option<(u64, u64)>,
) -> bool {
    let event_key_match = scanned_key.inner().as_slice() == provided_key.as_bytes();
    let module_address_match = match provided_address {
        Some(addr) => addr.as_slice() == scanned_address,
        None => true, // If module_address is not provided, always true
    };
    let txn_num_match = match provided_txn_range {
        Some(txn_range) => scanned_txn_num >= txn_range.0 && scanned_txn_num <= txn_range.1,
        None => true, // If transaction_num is not provided, always true
    };
    event_key_match && module_address_match && txn_num_match
}

pub(crate) fn extract_module_address_key_from_str(module_address: &str) -> anyhow::Result<Vec<u8>> {
    AddressBech32::from_str(module_address)
        .context("Failed to parse address from string")
        .and_then(|addr| {
            addr.try_to_vec()
                .context("Failed to convert bech32 address to bytes")
        })
}

pub(crate) fn get_events_by_key_helper<
    E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>,
>(
    ledger_db: &LedgerDB,
    event_key: &str,
    module_address: Option<&str>,
    txn_range: Option<(u64, u64)>,
    num_events: usize,
    next: Option<&str>,
) -> Result<PaginatedEventResponse, Error> {
    let module_address_vec = match module_address {
        None => vec![],
        Some(module_address) => extract_module_address_key_from_str(module_address)?,
    };

    let scan_key_start = match next {
        Some(start_key) => {
            let key_bytes = hex::decode(start_key)?;
            let composite_key: (EventKey, Vec<u8>, TxNumber, EventNumber) =
                BorshDeserialize::try_from_slice(&key_bytes)?;
            composite_key
        }
        None => (
            EventKey::new(event_key.as_bytes()),
            module_address_vec.clone(),
            TxNumber(txn_range.unwrap_or((0u64, 0u64)).0),
            EventNumber(0u64),
        ),
    };

    let paginated_query_response = ledger_db
        .db
        .get_n_from_first_match::<EventByKey>(&scan_key_start, num_events)?;

    let (event_keys, next_key) = (
        paginated_query_response.key_value,
        paginated_query_response.next,
    );
    let event_keys: Vec<((EventKey, ModuleAddress, TxNumber, EventNumber), ())> = event_keys
        .into_iter()
        .filter(|((e_key, m_address, t_num, _), _)| {
            event_match_helper(
                e_key,
                event_key,
                m_address,
                &module_address.map(|_| module_address_vec.clone()),
                t_num.0,
                txn_range,
            )
        })
        .collect();

    let event_ids: Vec<EventIdentifier> = event_keys
        .into_iter()
        .map(|(k, _)| EventIdentifier::Number(k.3 .0))
        .collect();
    let events_response: Vec<EventResponse> = ledger_db
        .get_events::<E>(&event_ids)?
        .into_iter()
        .flatten()
        .collect();
    let next = next_key
        .and_then(|next_key| {
            if !event_match_helper(
                &next_key.0,
                event_key,
                next_key.1.as_slice(),
                &module_address.map(|_| module_address_vec.clone()),
                next_key.2 .0,
                txn_range,
            ) {
                None
            } else {
                Some(next_key)
            }
        })
        .map(|next_key| next_key.try_to_vec().map(hex::encode))
        .transpose()?;
    Ok(PaginatedEventResponse {
        events_response,
        next,
    })
}

pub(crate) fn get_events_by_module_address_helper<
    E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>,
>(
    ledger_db: &LedgerDB,
    module_address: &str,
    num_events: usize,
    next: Option<&str>,
) -> Result<PaginatedEventResponse, Error> {
    let module_address_vec = extract_module_address_key_from_str(module_address)
        .with_context(|| format!("Could not deserialize module_address: {}", module_address))?;

    let scan_key_start = match next {
        Some(start_key) => {
            let key_bytes = hex::decode(start_key)?;
            let composite_key: (Vec<u8>, TxNumber, EventNumber) =
                BorshDeserialize::try_from_slice(&key_bytes)?;
            composite_key
        }
        None => (
            module_address_vec.clone(),
            TxNumber(0u64),
            EventNumber(0u64),
        ),
    };

    let paginated_query_response = ledger_db
        .db
        .get_n_from_first_match::<EventByModuleAddress>(&scan_key_start, num_events)?;

    let (event_keys, next_key) = (
        paginated_query_response.key_value,
        paginated_query_response.next,
    );

    let event_keys: Vec<((ModuleAddress, TxNumber, EventNumber), ())> = event_keys
        .into_iter()
        .filter(|((m_address, _, _), _)| m_address.as_slice() == module_address_vec.as_slice())
        .collect();

    let event_ids: Vec<EventIdentifier> = event_keys
        .into_iter()
        .map(|(k, _)| EventIdentifier::Number(k.2 .0))
        .collect();

    let events_response: Vec<EventResponse> = ledger_db
        .get_events::<E>(&event_ids)?
        .into_iter()
        .flatten()
        .collect();

    let next = next_key
        .map(|next_key| next_key.try_to_vec().map(hex::encode))
        .transpose()?;

    Ok(PaginatedEventResponse {
        events_response,
        next,
    })
}
