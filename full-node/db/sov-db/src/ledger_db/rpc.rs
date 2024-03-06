use anyhow::{ensure, Context, Error};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::de::DeserializeOwned;
use sov_modules_core::common::AddressBech32;
use sov_rollup_interface::rpc::{
    AggregatedProofResponse, BatchIdAndOffset, BatchIdentifier, BatchResponse, EventIdentifier,
    EventResponse, ItemOrHash, LedgerRpcProvider, PaginatedEventResponse, QueryMode,
    SlotIdAndOffset, SlotIdentifier, SlotResponse, TxIdAndOffset, TxIdentifier, TxResponse,
};
use sov_rollup_interface::zk::aggregated_proof::AggregatedProofData;
use tokio::sync::broadcast::Receiver;

use crate::ledger_db::event_helper::{
    get_events_by_key_helper, get_events_by_module_address_helper,
};
use crate::schema::tables::{
    BatchByHash, BatchByNumber, EventByNumber, ProofByUniqueId, SlotByHash, SlotByNumber, TxByHash,
    TxByNumber,
};
use crate::schema::types::{
    BatchNumber, EventNumber, SlotNumber, StoredBatch, StoredSlot, TxNumber,
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
        txn_range: Option<(u64, u64)>,
        num_events: usize,
        next: Option<&str>,
    ) -> Result<PaginatedEventResponse, Error> {
        get_events_by_key_helper::<E>(self, event_key, module_address, txn_range, num_events, next)
    }

    fn get_events_by_module_address<
        E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>,
    >(
        &self,
        module_address: &str,
        num_events: usize,
        next: Option<&str>,
    ) -> Result<PaginatedEventResponse, Error> {
        get_events_by_module_address_helper::<E>(self, module_address, num_events, next)
    }

    fn get_events_by_slot_range_key<
        E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>,
    >(
        &self,
        event_key: &str,
        module_address: &str,
        slot_height_start: u64,
        slot_height_end: u64,
        num_events: usize,
        next: Option<&str>,
    ) -> Result<PaginatedEventResponse, Error> {
        let (txn_range, next_key) = match next {
            None => {
                let slots_result: Vec<StoredSlot> = [slot_height_start, slot_height_end]
                    .into_iter()
                    .map(|slot_num| {
                        self.db
                            .read::<SlotByNumber>(&SlotNumber(slot_num))
                            .with_context(|| {
                                format!("Failed to query slot with number: {}", slot_num)
                            })
                            .and_then(|slot_opt| {
                                slot_opt.with_context(|| {
                                    format!(
                                        "Slot with number: {} does not exist in storage",
                                        slot_num
                                    )
                                })
                            })
                    })
                    .collect::<Result<Vec<StoredSlot>, _>>()?;

                let batch_start_num = slots_result[0].batches.start;
                let batch_end_num = slots_result[1].batches.end;

                ensure!(batch_end_num.0 - batch_start_num.0 < MAX_BATCHES_PER_REQUEST);

                let batches_result: Vec<StoredBatch> = [batch_start_num, batch_end_num]
                    .into_iter()
                    .map(|batch_num| {
                        self.db
                            .read::<BatchByNumber>(&batch_num)
                            .with_context(|| {
                                format!("Failed to query batch with number: {}", batch_num.0)
                            })
                            .and_then(|slot_opt| {
                                slot_opt.with_context(|| {
                                    format!(
                                        "Batch with number: {} does not exist in storage",
                                        batch_num.0
                                    )
                                })
                            })
                    })
                    .collect::<Result<Vec<StoredBatch>, _>>()?;

                let txn_start_num = batches_result[0].txs.start;
                let txn_end_num = batches_result[1].txs.end;
                ((txn_start_num.0, txn_end_num.0), None)
            }
            Some(wrapped_next) => {
                let key_bytes = hex::decode(wrapped_next)?;
                let composite_key: ((u64, u64), String) =
                    BorshDeserialize::try_from_slice(&key_bytes)?;
                (composite_key.0, Some(composite_key.1))
            }
        };

        let paginated_query_response = self.get_events_by_key::<E>(
            event_key,
            Some(module_address),
            Some((txn_range.0, txn_range.1)),
            num_events,
            next_key.as_deref(),
        )?;
        let (event_response, next_key) = (
            paginated_query_response.events_response,
            paginated_query_response.next,
        );
        let re_encoded_next = next_key
            .and_then(|inner_next| (txn_range, inner_next).try_to_vec().ok().map(hex::encode));

        Ok(PaginatedEventResponse {
            events_response: event_response,
            next: re_encoded_next,
        })
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
        let events_response = self
            .get_events_by_txn_number::<E>(tid.0)
            .with_context(|| format!("Failed to query txn with hash: {}", txn_hash))?;
        Ok(events_response)
    }

    fn get_events_by_txn_number<E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>>(
        &self,
        txn_num: u64,
    ) -> Result<Vec<EventResponse>, Error> {
        let stored_txn = self
            .db
            .read::<TxByNumber>(&TxNumber(txn_num))
            .with_context(|| format!("Failed to query txn num: {} from storage", txn_num))?
            .with_context(|| format!("Txn num: {} does not exist in storage", txn_num))?;
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
        let agg_proof_data = self.db.get_largest::<ProofByUniqueId>();

        match agg_proof_data? {
            Some(data) => {
                let proof = AggregatedProofData::try_from_slice(&data.1.proof)?;
                Ok(Some(AggregatedProofResponse { proof }))
            }
            None => Ok(None),
        }
    }

    fn subscribe_slots(&self) -> Receiver<u64> {
        self.slot_subscriptions.subscribe()
    }

    fn subscribe_proof_saved(&self) -> Receiver<AggregatedProofResponse> {
        self.proof_subscriptions.subscribe()
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

    use borsh::BorshSerialize;
    use rand::Rng;
    use sov_mock_da::{MockBlob, MockBlock};
    use sov_modules_api::utils::generate_address;
    use sov_modules_api::AddressBech32;
    use sov_rollup_interface::rpc::LedgerRpcProvider;
    use sov_rollup_interface::zk::aggregated_proof::{
        AggregatedProofData, AggregatedProofPublicInput, CodeCommitment,
    };
    use sov_schema_db::cache::cache_container::CacheContainer;
    use sov_schema_db::cache::cache_db::CacheDb;
    use sov_schema_db::SchemaBatch;
    use sov_test_utils::TestSpec;

    use crate::ledger_db::event_test_helper::{
        find_event_details, generate_events, TestEvent, FIXED_EVENT_KEY, MAX_NUM_EVENTS_FIXED_KEY,
        NUM_EVENTS_PER_TXN, NUM_MODULES, NUM_TXNS_PER_MODULE,
    };
    use crate::ledger_db::{LedgerDB, SlotCommit};
    use crate::schema::types::StoredAggregatedProof;

    #[test]
    fn test_slot_subscription() {
        let ledger_db = create_ledger();

        let mut rx = ledger_db.subscribe_slots();
        ledger_db
            .commit_slot(SlotCommit::<_, MockBlob, Vec<u8>>::new(MockBlock::default()))
            .unwrap();

        assert_eq!(rx.blocking_recv().unwrap(), 0);
    }

    #[test]
    fn test_get_events() {
        let ledger_db = create_ledger();
        let mut schema_batch = SchemaBatch::new();

        // Load events
        let event_count = generate_events(
            &ledger_db,
            &mut schema_batch,
            NUM_MODULES,
            NUM_TXNS_PER_MODULE,
            NUM_EVENTS_PER_TXN,
            MAX_NUM_EVENTS_FIXED_KEY,
        );

        ledger_db.db.write_many(schema_batch).unwrap();

        let mut rng = rand::thread_rng();

        // get_event_by_number
        let event_num = rng.gen_range(1..event_count) as u64;
        let event_response = ledger_db
            .get_event_by_number::<TestEvent>(event_num)
            .unwrap();
        assert!(event_response.is_some());
        let event = event_response.unwrap();
        let (_txn_number, module_number) = find_event_details(
            event_num,
            NUM_MODULES,
            NUM_TXNS_PER_MODULE,
            NUM_EVENTS_PER_TXN,
        );
        assert_eq!(event.module_name, format!("module_{}", module_number));
        assert_eq!(event.event_value, event_num + 1);
    }

    #[test]
    fn test_get_events_by_key() {
        let ledger_db = create_ledger();
        let mut schema_batch = SchemaBatch::new();

        // Load events
        let event_count = generate_events(
            &ledger_db,
            &mut schema_batch,
            NUM_MODULES,
            NUM_TXNS_PER_MODULE,
            NUM_EVENTS_PER_TXN,
            MAX_NUM_EVENTS_FIXED_KEY,
        );

        ledger_db.db.write_many(schema_batch).unwrap();

        let mut rng = rand::thread_rng();

        // single get_events_by_key
        let event_num = rng.gen_range(MAX_NUM_EVENTS_FIXED_KEY..event_count) as u64;
        let event_key = format!("key_{}", event_num);
        let (_txn_number, module_number) = find_event_details(
            event_num,
            NUM_MODULES,
            NUM_TXNS_PER_MODULE,
            NUM_EVENTS_PER_TXN,
        );
        let expected_module_name = format!("module_{}", module_number);
        let expected_module_address =
            AddressBech32::from(generate_address::<TestSpec>(&expected_module_name)).to_string();
        let event_response =
            ledger_db.get_events_by_key::<TestEvent>(&event_key, None, None, 1, None);

        assert!(event_response.is_ok());
        let event_response = event_response.unwrap();
        assert!(event_response.next.is_none());
        assert_eq!(event_response.events_response.len(), 1);
        assert_eq!(event_response.events_response[0].event_value, event_num + 1);
        assert_eq!(
            event_response.events_response[0].module_address,
            expected_module_address
        );
    }

    #[test]
    fn test_get_events_by_key_pagination() {
        let ledger_db = create_ledger();
        let mut schema_batch = SchemaBatch::new();

        // Load events
        let _event_count = generate_events(
            &ledger_db,
            &mut schema_batch,
            NUM_MODULES,
            NUM_TXNS_PER_MODULE,
            NUM_EVENTS_PER_TXN,
            MAX_NUM_EVENTS_FIXED_KEY,
        );

        ledger_db.db.write_many(schema_batch).unwrap();

        // choosing 7 because more non-standard, better for boundary cases
        let num_events_per_page = 7;
        // Because events with num > MAX_NUM_EVENTS_FIXED_KEY have unique keys, we test pagination for events smaller than that
        let mut next_key = None;
        let mut event_num = 0;
        let mut num_events_fetched = 0;
        loop {
            let event_response = ledger_db
                .get_events_by_key::<TestEvent>(
                    FIXED_EVENT_KEY,
                    None,
                    None,
                    num_events_per_page,
                    next_key.as_deref(),
                )
                .unwrap();

            num_events_fetched += event_response.events_response.len();

            for e in &event_response.events_response {
                // increment event_num for each event fetched
                event_num += 1;
                // our test data creates the value of an event to the event number + 1
                assert_eq!(e.event_value, event_num + 1);
                let (_, module_number) = find_event_details(
                    event_num,
                    NUM_MODULES,
                    NUM_TXNS_PER_MODULE,
                    NUM_EVENTS_PER_TXN,
                );
                let expected_module_name = format!("module_{}", module_number);
                let expected_module_address =
                    AddressBech32::from(generate_address::<TestSpec>(&expected_module_name))
                        .to_string();
                assert_eq!(e.module_address, expected_module_address);
            }

            // more events remaining
            if event_response.next.is_some() {
                next_key = event_response.next;
            } else {
                // event key ends
                // we need a different set of assertions for the final case
                let num_events_last_page = event_response.events_response.len();
                assert_eq!(num_events_fetched, MAX_NUM_EVENTS_FIXED_KEY);
                assert_eq!(event_num as usize, MAX_NUM_EVENTS_FIXED_KEY);
                assert_eq!(
                    event_response.events_response[num_events_last_page - 1].event_value,
                    MAX_NUM_EVENTS_FIXED_KEY + 1
                );
                break;
            }
        }

        // next event after final event should have unique key
        event_num += 1;
        let event_key = format!("key_{}", event_num);
        let (_txn_number, module_number) = find_event_details(
            event_num,
            NUM_MODULES,
            NUM_TXNS_PER_MODULE,
            NUM_EVENTS_PER_TXN,
        );
        let expected_module_name = format!("module_{}", module_number);
        let expected_module_address =
            AddressBech32::from(generate_address::<TestSpec>(&expected_module_name)).to_string();
        let event_response =
            ledger_db.get_events_by_key::<TestEvent>(&event_key, None, None, 1, None);

        assert!(event_response.is_ok());
        let event_response = event_response.unwrap();
        assert!(event_response.next.is_none());
        assert_eq!(event_response.events_response.len(), 1);
        assert_eq!(event_response.events_response[0].event_value, event_num + 1);
        assert_eq!(
            event_response.events_response[0].module_address,
            expected_module_address
        );
    }

    #[test]
    fn test_get_events_by_key_module_address() {
        let ledger_db = create_ledger();
        let mut schema_batch = SchemaBatch::new();

        // Load events
        let _event_count = generate_events(
            &ledger_db,
            &mut schema_batch,
            NUM_MODULES,
            NUM_TXNS_PER_MODULE,
            NUM_EVENTS_PER_TXN,
            MAX_NUM_EVENTS_FIXED_KEY,
        );

        ledger_db.db.write_many(schema_batch).unwrap();
        // fetch key and by module address
        // based on MAX_NUM_EVENTS_FIXED_KEY, key should switch to unique in the second module, so this would account for boundary as well
        let num_events_per_page = 17;
        let module_number = 2;
        let module_name = format!("module_{}", module_number);
        let module_address =
            AddressBech32::from(generate_address::<TestSpec>(&module_name)).to_string();

        let mut next_key = None;
        // start event number for a specific module
        let mut event_num = (module_number - 1) * NUM_EVENTS_PER_TXN * NUM_TXNS_PER_MODULE;
        let mut num_events_fetched = 0;
        loop {
            let event_response = ledger_db
                .get_events_by_key::<TestEvent>(
                    FIXED_EVENT_KEY,
                    Some(&module_address),
                    None,
                    num_events_per_page,
                    next_key.as_deref(),
                )
                .unwrap();

            for e in event_response.events_response {
                num_events_fetched += 1;
                event_num += 1;
                assert_eq!(e.event_value, event_num + 1);
                assert_eq!(e.module_address, module_address);
            }

            if event_response.next.is_some() {
                next_key = event_response.next;
            } else {
                break;
            }
        }

        // number of events fetched should be MAX_NUM_EVENTS_FIXED_KEY - number of events in previous modules.
        // since we're only fetching events for a specific module address.
        assert_eq!(
            num_events_fetched,
            MAX_NUM_EVENTS_FIXED_KEY
                - (module_number - 1) * NUM_EVENTS_PER_TXN * NUM_TXNS_PER_MODULE
        );
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

    #[test]
    fn test_save_aggregated_proof() {
        let ledger_db = create_ledger();
        let _rx = ledger_db.proof_subscriptions.subscribe();

        let proof_from_db = ledger_db.get_latest_aggregated_proof().unwrap();
        assert_eq!(None, proof_from_db);

        for i in 0..10 {
            let proof = AggregatedProofData::new(AggregatedProofPublicInput {
                initial_slot_number: i as u64,
                final_slot_number: i as u64,
                initial_state_root: vec![i],
                final_state_root: vec![i + 1],
                initial_slot_hash: vec![i + 2],
                final_slot_hash: vec![i + 3],
                code_commitment: CodeCommitment::default(),
            });

            let agg_proof = StoredAggregatedProof {
                proof: proof.try_to_vec().unwrap(),
            };

            ledger_db
                .save_finalized_aggregated_proof(agg_proof.clone())
                .unwrap();

            let proof_from_db = ledger_db.get_latest_aggregated_proof().unwrap().unwrap();
            assert_eq!(proof, proof_from_db.proof);
        }
    }
}
