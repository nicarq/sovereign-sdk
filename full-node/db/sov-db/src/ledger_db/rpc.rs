use std::str::FromStr;

use anyhow::{Context, Error};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::de::DeserializeOwned;
use sov_modules_core::common::AddressBech32;
use sov_rollup_interface::rpc::{
    AggregatedProofResponse, BatchIdAndOffset, BatchIdentifier, BatchResponse, EventIdentifier,
    EventResponse, ItemOrHash, LedgerRpcProvider, PaginatedEventResponse, QueryMode,
    SlotIdAndOffset, SlotIdentifier, SlotResponse, TxIdAndOffset, TxIdentifier, TxResponse,
};
use sov_rollup_interface::stf::EventKey;
use tokio::sync::broadcast::Receiver;

use crate::schema::tables::{
    BatchByHash, BatchByNumber, EventByKey, EventByModuleAddress, EventByNumber, ProofByUniqueId,
    SlotByHash, SlotByNumber, TxByHash, TxByNumber,
};
use crate::schema::types::{
    BatchNumber, EventNumber, ModuleAddress, SlotNumber, StoredBatch, StoredSlot, TxNumber,
};

/// The maximum number of slots that can be requested in a single RPC range query
const MAX_SLOTS_PER_REQUEST: u64 = 10;
/// The maximum number of batches that can be requested in a single RPC range query
const MAX_BATCHES_PER_REQUEST: u64 = 20;
/// The maximum number of transactions that can be requested in a single RPC range query
const MAX_TRANSACTIONS_PER_REQUEST: u64 = 100;
/// The maximum number of events that can be requested in a single RPC range query
const MAX_EVENTS_PER_REQUEST: u64 = 500;

use super::LedgerDB;

fn event_match_helper(
    scanned_key: &EventKey,
    provided_key: &str,
    scanned_address: &[u8],
    provided_address: &Option<Vec<u8>>,
) -> bool {
    let event_key_match = scanned_key.inner().as_slice() == provided_key.as_bytes();
    let module_address_match = match provided_address {
        Some(addr) => addr.as_slice() == scanned_address,
        None => true, // If module_address is not provided, always true
    };
    event_key_match && module_address_match
}

impl LedgerRpcProvider for LedgerDB {
    fn get_head<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        query_mode: QueryMode,
    ) -> Result<Option<SlotResponse<B, T>>, anyhow::Error> {
        let next_ids = self.get_next_items_numbers();
        let next_slot = next_ids.slot_number;

        let head_number = next_slot.saturating_sub(1);

        if let Some(stored_slot) = self
            .db
            .read::<SlotByNumber>(&SlotNumber(next_slot.saturating_sub(1)))?
        {
            return Ok(Some(self.populate_slot_response(
                head_number,
                stored_slot,
                query_mode,
            )?));
        }
        Ok(None)
    }

    fn get_slots<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        slot_ids: &[SlotIdentifier],
        query_mode: QueryMode,
    ) -> Result<Vec<Option<SlotResponse<B, T>>>, anyhow::Error> {
        anyhow::ensure!(
            slot_ids.len() <= MAX_SLOTS_PER_REQUEST as usize,
            "requested too many slots. Requested: {}. Max: {}",
            slot_ids.len(),
            MAX_SLOTS_PER_REQUEST
        );
        // TODO: https://github.com/Sovereign-Labs/sovereign-sdk/issues/191 Sort the input
        //      and use an iterator instead of querying for each slot individually
        let mut out = Vec::with_capacity(slot_ids.len());
        for slot_id in slot_ids {
            let slot_num = self.resolve_slot_identifier(slot_id)?;
            out.push(match slot_num {
                Some(num) => {
                    if let Some(stored_slot) = self.db.read::<SlotByNumber>(&num)? {
                        Some(self.populate_slot_response(num.into(), stored_slot, query_mode)?)
                    } else {
                        None
                    }
                }
                None => None,
            })
        }
        Ok(out)
    }

    fn get_batches<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        batch_ids: &[BatchIdentifier],
        query_mode: QueryMode,
    ) -> Result<Vec<Option<BatchResponse<B, T>>>, anyhow::Error> {
        anyhow::ensure!(
            batch_ids.len() <= MAX_BATCHES_PER_REQUEST as usize,
            "requested too many batches. Requested: {}. Max: {}",
            batch_ids.len(),
            MAX_BATCHES_PER_REQUEST
        );
        // TODO: https://github.com/Sovereign-Labs/sovereign-sdk/issues/191 Sort the input
        //      and use an iterator instead of querying for each slot individually
        let mut out = Vec::with_capacity(batch_ids.len());
        for batch_id in batch_ids {
            let batch_num = self.resolve_batch_identifier(batch_id)?;
            out.push(match batch_num {
                Some(num) => {
                    if let Some(stored_batch) = self.db.read::<BatchByNumber>(&num)? {
                        Some(self.populate_batch_response(stored_batch, query_mode)?)
                    } else {
                        None
                    }
                }
                None => None,
            })
        }
        Ok(out)
    }

    fn get_transactions<T: DeserializeOwned>(
        &self,
        tx_ids: &[TxIdentifier],
        _query_mode: QueryMode,
    ) -> Result<Vec<Option<TxResponse<T>>>, anyhow::Error> {
        anyhow::ensure!(
            tx_ids.len() <= MAX_TRANSACTIONS_PER_REQUEST as usize,
            "requested too many transactions. Requested: {}. Max: {}",
            tx_ids.len(),
            MAX_TRANSACTIONS_PER_REQUEST
        );
        // TODO: https://github.com/Sovereign-Labs/sovereign-sdk/issues/191 Sort the input
        //      and use an iterator instead of querying for each slot individually
        let mut out: Vec<Option<TxResponse<T>>> = Vec::with_capacity(tx_ids.len());
        for id in tx_ids {
            let num = self.resolve_tx_identifier(id)?;
            out.push(match num {
                Some(num) => {
                    if let Some(tx) = self.db.read::<TxByNumber>(&num)? {
                        Some(tx.try_into()?)
                    } else {
                        None
                    }
                }
                None => None,
            })
        }
        Ok(out)
    }

    fn get_events<E: borsh::BorshDeserialize + Into<sov_rollup_interface::rpc::Event>>(
        &self,
        event_ids: &[EventIdentifier],
    ) -> Result<Vec<Option<EventResponse>>, anyhow::Error> {
        anyhow::ensure!(
            event_ids.len() <= MAX_EVENTS_PER_REQUEST as usize,
            "requested too many events. Requested: {}. Max: {}",
            event_ids.len(),
            MAX_EVENTS_PER_REQUEST
        );
        // TODO: Sort the input and use an iterator instead of querying for each slot individually
        // https://github.com/Sovereign-Labs/sovereign-sdk/issues/191
        let mut out = Vec::with_capacity(event_ids.len());
        for id in event_ids {
            let num = self.resolve_event_identifier(id)?;
            out.push(match num {
                Some(num) => {
                    self.db
                        .read::<EventByNumber>(&num)?
                        .map(|serialized_event| {
                            match E::deserialize(&mut serialized_event.value().inner().as_slice()) {
                                // serde_json::to_value is from the custom serialize impl which
                                // matches for the specific event
                                // and then converts that event to json value, instead of the outer RuntimeEvent
                                Ok(event) => {
                                    let module_event: sov_rollup_interface::rpc::Event =
                                        event.into();
                                    Some(EventResponse {
                                        event_value: module_event.event_value,
                                        module_name: module_event.module_name,
                                        module_address: AddressBech32::try_from_slice(
                                            serialized_event.module_address().inner().as_slice(),
                                        )
                                        .unwrap()
                                        .to_string(),
                                    })
                                }
                                Err(_) => None,
                            }
                        })
                        .unwrap_or(None)
                }
                None => None,
            })
        }
        Ok(out)
    }

    // Get X by hash
    fn get_slot_by_hash<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        hash: &[u8; 32],
        query_mode: QueryMode,
    ) -> Result<Option<SlotResponse<B, T>>, anyhow::Error> {
        self.get_slots(&[SlotIdentifier::Hash(*hash)], query_mode)
            .map(|mut batches: Vec<Option<SlotResponse<B, T>>>| batches.pop().unwrap_or(None))
    }

    fn get_batch_by_hash<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        hash: &[u8; 32],
        query_mode: QueryMode,
    ) -> Result<Option<BatchResponse<B, T>>, anyhow::Error> {
        self.get_batches(&[BatchIdentifier::Hash(*hash)], query_mode)
            .map(|mut batches: Vec<Option<BatchResponse<B, T>>>| batches.pop().unwrap_or(None))
    }

    fn get_tx_by_hash<T: DeserializeOwned>(
        &self,
        hash: &[u8; 32],
        query_mode: QueryMode,
    ) -> Result<Option<TxResponse<T>>, anyhow::Error> {
        self.get_transactions(&[TxIdentifier::Hash(*hash)], query_mode)
            .map(|mut txs: Vec<Option<TxResponse<T>>>| txs.pop().unwrap_or(None))
    }

    // Get X by number
    fn get_slot_by_number<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        number: u64,
        query_mode: QueryMode,
    ) -> Result<Option<SlotResponse<B, T>>, anyhow::Error> {
        self.get_slots(&[SlotIdentifier::Number(number)], query_mode)
            .map(|mut slots: Vec<Option<SlotResponse<B, T>>>| slots.pop().unwrap_or(None))
    }

    fn get_batch_by_number<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        number: u64,
        query_mode: QueryMode,
    ) -> Result<Option<BatchResponse<B, T>>, anyhow::Error> {
        self.get_batches(&[BatchIdentifier::Number(number)], query_mode)
            .map(|mut slots| slots.pop().unwrap_or(None))
    }

    fn get_event_by_number<E: borsh::BorshDeserialize + Into<sov_rollup_interface::rpc::Event>>(
        &self,
        number: u64,
    ) -> Result<Option<EventResponse>, anyhow::Error> {
        self.get_events::<E>(&[EventIdentifier::Number(number)])
            .map(|mut events| events.pop().flatten())
    }

    fn get_events_by_key<E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>>(
        &self,
        event_key: &str,
        module_address: Option<&str>,
        num_events: usize,
        next: Option<&str>,
    ) -> Result<PaginatedEventResponse, Error> {
        let module_address_vec = module_address
            .map(|bech32_string| {
                AddressBech32::from_str(bech32_string)
                    .context("Failed to parse address from string")
                    .and_then(|addr| {
                        addr.try_to_vec()
                            .context("Failed to convert bech32 address to bytes")
                    })
            })
            .transpose()?
            .unwrap_or(vec![]);

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
                TxNumber(0u64),
                EventNumber(0u64),
            ),
        };

        let paginated_query_response = self
            .db
            .get_n_from_first_match::<EventByKey>(&scan_key_start, num_events)?;

        let (event_keys, next_key) = (
            paginated_query_response.key_value,
            paginated_query_response.next,
        );
        let event_keys: Vec<((EventKey, ModuleAddress, TxNumber, EventNumber), ())> = event_keys
            .into_iter()
            .filter(|((e_key, m_address, _, _), _)| {
                event_match_helper(
                    e_key,
                    event_key,
                    m_address,
                    &module_address.map(|_| module_address_vec.clone()),
                )
            })
            .collect();

        let event_ids: Vec<EventIdentifier> = event_keys
            .into_iter()
            .map(|(k, _)| EventIdentifier::Number(k.3 .0))
            .collect();
        let events_response: Vec<EventResponse> = self
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

    fn get_events_by_module_address<
        E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>,
    >(
        &self,
        module_address: &str,
        num_events: usize,
        next: Option<&str>,
    ) -> Result<PaginatedEventResponse, Error> {
        let module_address_vec = AddressBech32::from_str(module_address)
            .context("Could not convert provided string to bech32")
            .and_then(|x| {
                x.try_to_vec()
                    .context("Could not convert bech32 address to bytes")
            })?;

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

        let paginated_query_response = self
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

        let events_response: Vec<EventResponse> = self
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

    fn get_events_by_slot_range_key<
        E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>,
    >(
        &self,
        _event_key: Option<&str>,
        _module_address: Option<&str>,
        _slot_height_start: usize,
        _slot_height_end: usize,
        _num_events: usize,
        _next: Option<&str>,
    ) -> Result<PaginatedEventResponse, Error> {
        todo!()
    }

    fn get_events_by_txn_hash<E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>>(
        &self,
        txn_hash: &str,
    ) -> Result<Vec<EventResponse>, Error> {
        let tx_vec = hex::decode(txn_hash)?;
        if tx_vec.len() != 32 {
            anyhow::bail!("Provided string does not match expected length of 32");
        }
        let mut tx_bytes = [0u8; 32];
        tx_bytes.copy_from_slice(&tx_vec);
        let tid = self
            .db
            .read::<TxByHash>(&tx_bytes)
            .with_context(|| format!("Failed to query txn with hash: {}", txn_hash))?
            .with_context(|| format!("Txn with hash: {} does not exist in storage", txn_hash))?;
        let stored_txn = self
            .db
            .read::<TxByNumber>(&tid)
            .with_context(|| format!("Failed to query txn num: {} from storage", tid.0))?
            .with_context(|| format!("Txn num: {} does not exist in storage", tid.0))?;
        // Can't map over stored_txn.events because no Step trait, so doing this manually
        // TODO: can we implement the Step trait
        // let event_ids: Vec<EventIdentifier> =
        //     stored_txn.events.map(EventIdentifier::Number).collect();

        let mut event_ids = Vec::new();
        let EventNumber(start) = stored_txn.events.start;
        let EventNumber(end) = stored_txn.events.end;
        for number in start..end {
            event_ids.push(EventIdentifier::Number(number));
        }

        let events_response: Vec<EventResponse> = self
            .get_events::<E>(&event_ids)?
            .into_iter()
            .flatten()
            .collect();
        Ok(events_response)
    }

    fn get_tx_by_number<T: DeserializeOwned>(
        &self,
        number: u64,
        query_mode: QueryMode,
    ) -> Result<Option<TxResponse<T>>, anyhow::Error> {
        self.get_transactions(&[TxIdentifier::Number(number)], query_mode)
            .map(|mut txs| txs.pop().unwrap_or(None))
    }

    fn get_slots_range<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        start: u64,
        end: u64,
        query_mode: QueryMode,
    ) -> Result<Vec<Option<SlotResponse<B, T>>>, anyhow::Error> {
        anyhow::ensure!(start <= end, "start must be <= end");
        anyhow::ensure!(
            end - start <= MAX_SLOTS_PER_REQUEST,
            "requested slot range too large. Max: {}",
            MAX_SLOTS_PER_REQUEST
        );
        let ids: Vec<_> = (start..=end).map(SlotIdentifier::Number).collect();
        self.get_slots(&ids, query_mode)
    }

    fn get_batches_range<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        start: u64,
        end: u64,
        query_mode: QueryMode,
    ) -> Result<Vec<Option<BatchResponse<B, T>>>, anyhow::Error> {
        anyhow::ensure!(start <= end, "start must be <= end");
        anyhow::ensure!(
            end - start <= MAX_BATCHES_PER_REQUEST,
            "requested batch range too large. Max: {}",
            MAX_BATCHES_PER_REQUEST
        );
        let ids: Vec<_> = (start..=end).map(BatchIdentifier::Number).collect();
        self.get_batches(&ids, query_mode)
    }

    fn get_transactions_range<T: DeserializeOwned>(
        &self,
        start: u64,
        end: u64,
        query_mode: QueryMode,
    ) -> Result<Vec<Option<TxResponse<T>>>, anyhow::Error> {
        anyhow::ensure!(start <= end, "start must be <= end");
        anyhow::ensure!(
            end - start <= MAX_TRANSACTIONS_PER_REQUEST,
            "requested transaction range too large. Max: {}",
            MAX_TRANSACTIONS_PER_REQUEST
        );
        let ids: Vec<_> = (start..=end).map(TxIdentifier::Number).collect();
        self.get_transactions(&ids, query_mode)
    }

    fn get_latest_aggregated_proof(&self) -> anyhow::Result<Option<AggregatedProofResponse>> {
        let agg_proof_data = self
            .db
            .get_largest::<ProofByUniqueId>()
            .map(|agg_proof_op| agg_proof_op.map(|p| p.1))?;

        Ok(agg_proof_data.map(|p| AggregatedProofResponse { proof: p.proof }))
    }
    fn subscribe_slots(&self) -> Result<Receiver<u64>, anyhow::Error> {
        Ok(self.slot_subscriptions.subscribe())
    }
}

impl LedgerDB {
    fn resolve_slot_identifier(
        &self,
        slot_id: &SlotIdentifier,
    ) -> Result<Option<SlotNumber>, anyhow::Error> {
        match slot_id {
            SlotIdentifier::Hash(hash) => self.db.read::<SlotByHash>(hash),
            SlotIdentifier::Number(num) => Ok(Some(SlotNumber(*num))),
        }
    }

    fn resolve_batch_identifier(
        &self,
        batch_id: &BatchIdentifier,
    ) -> Result<Option<BatchNumber>, anyhow::Error> {
        match batch_id {
            BatchIdentifier::Hash(hash) => self.db.read::<BatchByHash>(hash),
            BatchIdentifier::Number(num) => Ok(Some(BatchNumber(*num))),
            BatchIdentifier::SlotIdAndOffset(SlotIdAndOffset { slot_id, offset }) => {
                if let Some(slot_num) = self.resolve_slot_identifier(slot_id)? {
                    Ok(self
                        .db
                        .read::<SlotByNumber>(&slot_num)?
                        .map(|slot: StoredSlot| BatchNumber(slot.batches.start.0 + offset)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn resolve_tx_identifier(
        &self,
        tx_id: &TxIdentifier,
    ) -> Result<Option<TxNumber>, anyhow::Error> {
        match tx_id {
            TxIdentifier::Hash(hash) => self.db.read::<TxByHash>(hash),
            TxIdentifier::Number(num) => Ok(Some(TxNumber(*num))),
            TxIdentifier::BatchIdAndOffset(BatchIdAndOffset { batch_id, offset }) => {
                if let Some(batch_num) = self.resolve_batch_identifier(batch_id)? {
                    Ok(self
                        .db
                        .read::<BatchByNumber>(&batch_num)?
                        .map(|batch: StoredBatch| TxNumber(batch.txs.start.0 + offset)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn resolve_event_identifier(
        &self,
        event_id: &EventIdentifier,
    ) -> Result<Option<EventNumber>, anyhow::Error> {
        match event_id {
            EventIdentifier::TxIdAndOffset(TxIdAndOffset { tx_id, offset }) => {
                if let Some(tx_num) = self.resolve_tx_identifier(tx_id)? {
                    Ok(self
                        .db
                        .read::<TxByNumber>(&tx_num)?
                        .map(|tx| EventNumber(tx.events.start.0 + offset)))
                } else {
                    Ok(None)
                }
            }
            EventIdentifier::Number(num) => Ok(Some(EventNumber(*num))),
            EventIdentifier::TxIdAndKey(_) => todo!(),
        }
    }

    fn populate_slot_response<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        number: u64,
        slot: StoredSlot,
        mode: QueryMode,
    ) -> Result<SlotResponse<B, T>, anyhow::Error> {
        Ok(match mode {
            QueryMode::Compact => SlotResponse {
                number,
                hash: slot.hash,
                batch_range: slot.batches.start.into()..slot.batches.end.into(),
                batches: None,
            },
            QueryMode::Standard => {
                let batches = self.get_batch_range(&slot.batches)?;
                let batch_hashes = Some(
                    batches
                        .into_iter()
                        .map(|batch| ItemOrHash::Hash(batch.hash))
                        .collect(),
                );
                SlotResponse {
                    number,
                    hash: slot.hash,
                    batch_range: slot.batches.start.into()..slot.batches.end.into(),
                    batches: batch_hashes,
                }
            }
            QueryMode::Full => {
                let num_batches = (slot.batches.end.0 - slot.batches.start.0) as usize;
                let mut batches = Vec::with_capacity(num_batches);
                for batch in self.get_batch_range(&slot.batches)? {
                    batches.push(ItemOrHash::Full(self.populate_batch_response(batch, mode)?));
                }

                SlotResponse {
                    number,
                    hash: slot.hash,
                    batch_range: slot.batches.start.into()..slot.batches.end.into(),
                    batches: Some(batches),
                }
            }
        })
    }

    fn populate_batch_response<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        batch: StoredBatch,
        mode: QueryMode,
    ) -> Result<BatchResponse<B, T>, anyhow::Error> {
        Ok(match mode {
            QueryMode::Compact => batch.try_into()?,

            QueryMode::Standard => {
                let txs = self.get_tx_range(&batch.txs)?;
                let tx_hashes = Some(
                    txs.into_iter()
                        .map(|tx| ItemOrHash::Hash(tx.hash))
                        .collect(),
                );

                let mut batch_response: BatchResponse<B, T> = batch.try_into()?;
                batch_response.txs = tx_hashes;
                batch_response
            }
            QueryMode::Full => {
                let num_txs = (batch.txs.end.0 - batch.txs.start.0) as usize;
                let mut txs = Vec::with_capacity(num_txs);
                for tx in self.get_tx_range(&batch.txs)? {
                    txs.push(ItemOrHash::Full(tx.try_into()?));
                }

                let mut batch_response: BatchResponse<B, T> = batch.try_into()?;
                batch_response.txs = Some(txs);
                batch_response
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use sov_mock_da::{MockBlob, MockBlock};
    use sov_rollup_interface::rpc::LedgerRpcProvider;
    use sov_schema_db::cache::cache_container::CacheContainer;
    use sov_schema_db::cache::cache_db::CacheDb;

    use crate::ledger_db::{LedgerDB, SlotCommit};
    use crate::schema::types::StoredAggregatedProof;
    #[test]
    fn test_slot_subscription() {
        let ledger_db = create_ledger();

        let mut rx = ledger_db.subscribe_slots().unwrap();
        ledger_db
            .commit_slot(SlotCommit::<_, MockBlob, Vec<u8>>::new(MockBlock::default()))
            .unwrap();

        assert_eq!(rx.blocking_recv().unwrap(), 1);
    }

    #[test]
    fn test_save_aggregated_proof() {
        let ledger_db = create_ledger();

        let proof_from_db = ledger_db.get_latest_aggregated_proof().unwrap();
        assert_eq!(None, proof_from_db);

        for i in 0..10 {
            let agg_proof = StoredAggregatedProof { proof: vec![i] };

            ledger_db
                .save_finalized_aggregated_proof(agg_proof.clone())
                .unwrap();

            let proof_from_db = ledger_db.get_latest_aggregated_proof().unwrap().unwrap();
            assert_eq!(agg_proof.proof, proof_from_db.proof)
        }
    }

    fn create_ledger() -> LedgerDB {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path();
        let db = LedgerDB::setup_schema_db(path).unwrap();
        let cache_container = Arc::new(RwLock::new(CacheContainer::new(
            db,
            Arc::new(RwLock::new(Default::default())).into(),
        )));
        let cache_db = CacheDb::new(0, cache_container.into());
        LedgerDB::with_cache_db(cache_db).unwrap()
    }
}
