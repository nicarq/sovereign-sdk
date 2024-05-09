use anyhow::{bail, Context, Error};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use sov_rollup_interface::rpc::{
    AggregatedProofResponse, BatchIdAndOffset, BatchIdentifier, BatchResponse, EventIdentifier,
    ItemOrHash, LedgerStateProvider, QueryMode, SlotIdAndOffset, SlotIdentifier, SlotResponse,
    TxIdAndOffset, TxIdentifier, TxResponse,
};
use sov_rollup_interface::stf::StoredEvent;
use tokio::sync::broadcast::Receiver;

use crate::ledger_db::rpc_constants::{
    MAX_BATCHES_PER_REQUEST, MAX_EVENTS_PER_REQUEST, MAX_SLOTS_PER_REQUEST,
    MAX_TRANSACTIONS_PER_REQUEST,
};
use crate::ledger_db::LedgerDb;
use crate::schema::tables::{
    BatchByHash, BatchByNumber, EventByNumber, ProofByUniqueId, SlotByHash, SlotByNumber, TxByHash,
    TxByNumber,
};
use crate::schema::types::{
    BatchNumber, EventNumber, SlotNumber, StoredBatch, StoredSlot, TxNumber,
};

#[async_trait]
impl LedgerStateProvider for LedgerDb {
    type Error = anyhow::Error;

    async fn get_head_slot_number(&self) -> Result<Option<u64>, Self::Error> {
        let next_ids = self.get_next_items_numbers();
        let next_slot = next_ids.slot_number;

        Ok(Some(next_slot.saturating_sub(1)))
    }

    async fn get_slots<B, T>(
        &self,
        slot_ids: &[SlotIdentifier],
        query_mode: QueryMode,
    ) -> Result<Vec<Option<SlotResponse<B, T>>>, Self::Error>
    where
        B: DeserializeOwned + Send + Sync,
        T: DeserializeOwned + Send + Sync,
    {
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
            let slot_num = self.resolve_slot_identifier(slot_id).await?;
            out.push(match slot_num {
                Some(num) => {
                    if let Some(stored_slot) = self.db.read::<SlotByNumber>(&SlotNumber(num))? {
                        Some(self.populate_slot_response(num, stored_slot, query_mode)?)
                    } else {
                        None
                    }
                }
                None => None,
            });
        }
        Ok(out)
    }

    async fn get_batches<B, T>(
        &self,
        batch_ids: &[BatchIdentifier],
        query_mode: QueryMode,
    ) -> Result<Vec<Option<BatchResponse<B, T>>>, Self::Error>
    where
        B: DeserializeOwned + Send + Sync,
        T: DeserializeOwned + Send + Sync,
    {
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
            let batch_num = self.resolve_batch_identifier(batch_id).await?;
            out.push(match batch_num {
                Some(num) => {
                    if let Some(stored_batch) = self.db.read::<BatchByNumber>(&BatchNumber(num))? {
                        Some(self.populate_batch_response(stored_batch, query_mode)?)
                    } else {
                        None
                    }
                }
                None => None,
            });
        }
        Ok(out)
    }

    async fn get_transactions<T>(
        &self,
        tx_ids: &[TxIdentifier],
        _query_mode: QueryMode,
    ) -> Result<Vec<Option<TxResponse<T>>>, Self::Error>
    where
        T: DeserializeOwned + Send + Sync,
    {
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
            let num = self.resolve_tx_identifier(id).await?;
            out.push(match num {
                Some(num) => {
                    if let Some(tx) = self.db.read::<TxByNumber>(&TxNumber(num))? {
                        Some(tx.try_into()?)
                    } else {
                        None
                    }
                }
                None => None,
            });
        }
        Ok(out)
    }

    async fn get_events<E>(
        &self,
        event_ids: &[EventIdentifier],
    ) -> Result<Vec<Option<E>>, Self::Error>
    where
        E: TryFrom<StoredEvent, Error = anyhow::Error> + Send + Sync,
    {
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
            let num = self.resolve_event_identifier(id).await?;
            out.push(
                match num {
                    Some(num) => self
                        .db
                        .read::<EventByNumber>(&EventNumber(num))?
                        .map(|serialized_event| serialized_event.try_into()),
                    None => None,
                }
                .transpose()?,
            );
        }
        Ok(out)
    }

    // Get X by hash
    async fn get_slot_by_hash<B, T>(
        &self,
        hash: &[u8; 32],
        query_mode: QueryMode,
    ) -> Result<Option<SlotResponse<B, T>>, anyhow::Error>
    where
        B: DeserializeOwned + Send + Sync,
        T: DeserializeOwned + Send + Sync,
    {
        self.get_slots(&[SlotIdentifier::Hash(*hash)], query_mode)
            .await
            .map(|mut batches: Vec<Option<SlotResponse<B, T>>>| batches.pop().unwrap_or(None))
    }

    async fn get_batch_by_hash<B, T>(
        &self,
        hash: &[u8; 32],
        query_mode: QueryMode,
    ) -> Result<Option<BatchResponse<B, T>>, anyhow::Error>
    where
        B: DeserializeOwned + Send + Sync,
        T: DeserializeOwned + Send + Sync,
    {
        self.get_batches(&[BatchIdentifier::Hash(*hash)], query_mode)
            .await
            .map(|mut batches: Vec<Option<BatchResponse<B, T>>>| batches.pop().unwrap_or(None))
    }

    async fn get_tx_by_hash<T>(
        &self,
        hash: &[u8; 32],
        query_mode: QueryMode,
    ) -> Result<Option<TxResponse<T>>, anyhow::Error>
    where
        T: DeserializeOwned + Send + Sync,
    {
        self.get_transactions(&[TxIdentifier::Hash(*hash)], query_mode)
            .await
            .map(|mut txs: Vec<Option<TxResponse<T>>>| txs.pop().unwrap_or(None))
    }

    // Get X by number
    async fn get_slot_by_number<B, T>(
        &self,
        number: u64,
        query_mode: QueryMode,
    ) -> Result<Option<SlotResponse<B, T>>, anyhow::Error>
    where
        B: DeserializeOwned + Send + Sync,
        T: DeserializeOwned + Send + Sync,
    {
        self.get_slots(&[SlotIdentifier::Number(number)], query_mode)
            .await
            .map(|mut slots: Vec<Option<SlotResponse<B, T>>>| slots.pop().unwrap_or(None))
    }

    async fn get_batch_by_number<B, T>(
        &self,
        number: u64,
        query_mode: QueryMode,
    ) -> Result<Option<BatchResponse<B, T>>, anyhow::Error>
    where
        B: DeserializeOwned + Send + Sync,
        T: DeserializeOwned + Send + Sync,
    {
        self.get_batches(&[BatchIdentifier::Number(number)], query_mode)
            .await
            .map(|mut slots| slots.pop().unwrap_or(None))
    }

    async fn get_event_by_number<E>(&self, number: u64) -> Result<Option<E>, anyhow::Error>
    where
        E: TryFrom<StoredEvent, Error = anyhow::Error> + Send + Sync,
    {
        self.get_events::<E>(&[EventIdentifier::Number(number)])
            .await
            .map(|mut events| events.pop().flatten())
    }

    async fn get_events_by_txn_hash<E>(&self, txn_hash: &[u8; 32]) -> Result<Vec<E>, Error>
    where
        E: TryFrom<StoredEvent, Error = anyhow::Error> + Send + Sync,
    {
        let tx_range = (*txn_hash, TxNumber(0))..(*txn_hash, TxNumber(u64::MAX));
        let tx_numbers = self
            .db
            .collect_in_range::<TxByHash, ([u8; 32], TxNumber)>(tx_range)
            .with_context(|| {
                format!("Failed to query txn with hash: 0x{}", hex::encode(txn_hash))
            })?;

        if tx_numbers.is_empty() {
            bail!(
                "Txn with hash: 0x{} does not exist in storage",
                hex::encode(txn_hash)
            )
        }

        let mut events_response = vec![];
        for ((_, tx_num), _) in tx_numbers {
            let events = self
                .get_events_by_txn_number::<E>(tx_num.0)
                .await
                .with_context(|| {
                    format!("Resolved transaction hash {} to tx number {}, but failed to resolve find the events for that number", hex::encode(txn_hash), tx_num.0)
                })?;
            events_response.extend(events.into_iter());
        }
        Ok(events_response)
    }

    async fn get_events_by_txn_number<E>(&self, txn_num: u64) -> Result<Vec<E>, Error>
    where
        E: TryFrom<StoredEvent, Error = anyhow::Error> + Send + Sync,
    {
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

        let events_response: Vec<E> = self
            .get_events::<E>(&event_ids)
            .await?
            .into_iter()
            .flatten()
            .collect();
        Ok(events_response)
    }

    async fn get_slots_range<B, T>(
        &self,
        start: u64,
        end: u64,
        query_mode: QueryMode,
    ) -> Result<Vec<Option<SlotResponse<B, T>>>, Self::Error>
    where
        B: DeserializeOwned + Send + Sync,
        T: DeserializeOwned + Send + Sync,
    {
        anyhow::ensure!(start <= end, "start must be <= end");
        anyhow::ensure!(
            end - start <= MAX_SLOTS_PER_REQUEST,
            "requested slot range too large. Max: {}",
            MAX_SLOTS_PER_REQUEST
        );
        let ids: Vec<_> = (start..=end).map(SlotIdentifier::Number).collect();
        self.get_slots(&ids, query_mode).await
    }

    async fn get_batches_range<B, T>(
        &self,
        start: u64,
        end: u64,
        query_mode: QueryMode,
    ) -> Result<Vec<Option<BatchResponse<B, T>>>, Self::Error>
    where
        B: DeserializeOwned + Send + Sync,
        T: DeserializeOwned + Send + Sync,
    {
        anyhow::ensure!(start <= end, "start must be <= end");
        anyhow::ensure!(
            end - start <= MAX_BATCHES_PER_REQUEST,
            "requested batch range too large. Max: {}",
            MAX_BATCHES_PER_REQUEST
        );
        let ids: Vec<_> = (start..=end).map(BatchIdentifier::Number).collect();
        self.get_batches(&ids, query_mode).await
    }

    async fn get_transactions_range<T>(
        &self,
        start: u64,
        end: u64,
        query_mode: QueryMode,
    ) -> Result<Vec<Option<TxResponse<T>>>, Self::Error>
    where
        T: DeserializeOwned + Send + Sync,
    {
        anyhow::ensure!(start <= end, "start must be <= end");
        anyhow::ensure!(
            end - start <= MAX_TRANSACTIONS_PER_REQUEST,
            "requested transaction range too large. Max: {}",
            MAX_TRANSACTIONS_PER_REQUEST
        );
        let ids: Vec<_> = (start..=end).map(TxIdentifier::Number).collect();
        self.get_transactions(&ids, query_mode).await
    }

    async fn resolve_slot_identifier(
        &self,
        slot_id: &SlotIdentifier,
    ) -> Result<Option<u64>, Self::Error> {
        match slot_id {
            SlotIdentifier::Hash(hash) => self
                .db
                .read::<SlotByHash>(hash)
                .map(|id_opt| id_opt.map(|id| id.0)),
            SlotIdentifier::Number(num) => Ok(Some(*num)),
        }
    }

    async fn resolve_batch_identifier(
        &self,
        batch_id: &BatchIdentifier,
    ) -> Result<Option<u64>, Self::Error> {
        match batch_id {
            BatchIdentifier::Hash(hash) => self
                .db
                .read::<BatchByHash>(hash)
                .map(|id_opt| id_opt.map(|id| id.0)),
            BatchIdentifier::Number(num) => Ok(Some(*num)),
            BatchIdentifier::SlotIdAndOffset(SlotIdAndOffset { slot_id, offset }) => {
                if let Some(slot_num) = self.resolve_slot_identifier(slot_id).await? {
                    Ok(self
                        .db
                        .read::<SlotByNumber>(&SlotNumber(slot_num))?
                        .map(|slot: StoredSlot| slot.batches.start.0 + offset))
                } else {
                    Ok(None)
                }
            }
        }
    }

    async fn resolve_tx_identifier(
        &self,
        tx_id: &TxIdentifier,
    ) -> Result<Option<u64>, Self::Error> {
        match tx_id {
            TxIdentifier::Hash(hash) => {
                // When someone queries for a single TX by hash, we assume they want the first one.
                // This heuristic is better than our old one (implicitly returning the latest instance), because
                // it's more likely that a transaction gets succeeds on its first inclusion than on a second one.
                // (This is because transactions with *future* nonces rarely get included, but transactions with
                // past nonces can get included easily by racing sequencers.)
                // TODO: Add an endpoint returning all tx numbers for a given hash so that the caller
                // can identify the instance they care about and query it by number
                // <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/518>
                let tx_range = (*hash, TxNumber(0))..(*hash, TxNumber(u64::MAX));
                let tx_numbers = self
                    .db
                    .collect_in_range::<TxByHash, ([u8; 32], TxNumber)>(tx_range)
                    .with_context(|| {
                        format!("Failed to query txn with hash: 0x{}", hex::encode(hash))
                    })?;
                Ok(tx_numbers.first().map(|((_, tx_num), _)| tx_num.0))
            }
            TxIdentifier::Number(num) => Ok(Some(*num)),
            TxIdentifier::BatchIdAndOffset(BatchIdAndOffset { batch_id, offset }) => {
                if let Some(batch_num) = self.resolve_batch_identifier(batch_id).await? {
                    Ok(self
                        .db
                        .read::<BatchByNumber>(&BatchNumber(batch_num))?
                        .map(|batch: StoredBatch| batch.txs.start.0 + offset))
                } else {
                    Ok(None)
                }
            }
        }
    }

    async fn resolve_event_identifier(
        &self,
        event_id: &EventIdentifier,
    ) -> Result<Option<u64>, Self::Error> {
        match event_id {
            EventIdentifier::TxIdAndOffset(TxIdAndOffset { tx_id, offset }) => {
                if let Some(tx_num) = self.resolve_tx_identifier(tx_id).await? {
                    Ok(self
                        .db
                        .read::<TxByNumber>(&TxNumber(tx_num))?
                        .map(|tx| tx.events.start.0 + offset))
                } else {
                    Ok(None)
                }
            }
            EventIdentifier::Number(num) => Ok(Some(*num)),
        }
    }

    async fn get_latest_aggregated_proof(&self) -> anyhow::Result<Option<AggregatedProofResponse>> {
        let agg_proof_data = self.db.get_largest::<ProofByUniqueId>();

        match agg_proof_data? {
            Some((_, proof)) => Ok(Some(AggregatedProofResponse { proof })),
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

impl LedgerDb {
    fn populate_slot_response<B: DeserializeOwned, T: DeserializeOwned>(
        &self,
        number: u64,
        slot: StoredSlot,
        mode: QueryMode,
    ) -> Result<SlotResponse<B, T>, anyhow::Error> {
        let state_root = slot.state_root.as_ref().to_vec();

        Ok(match mode {
            QueryMode::Compact => SlotResponse {
                number,
                hash: slot.hash,
                state_root,
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
                    state_root,
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
                    state_root,
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

    use rockbound::cache::cache_container::CacheContainer;
    use rockbound::cache::cache_db::CacheDb;
    use sov_mock_da::{MockBlob, MockBlock};
    use sov_mock_zkvm::MockZkvm;
    use sov_rollup_interface::rpc::LedgerStateProvider;
    use sov_rollup_interface::zk::aggregated_proof::{
        AggregatedProof, AggregatedProofPublicData, CodeCommitment, SerializedAggregatedProof,
    };

    use crate::ledger_db::{LedgerDb, SlotCommit};

    #[test]
    fn test_slot_subscription() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ledger_db = create_ledger(temp_dir.path());

        let mut rx = ledger_db.subscribe_slots();
        ledger_db
            .commit_slot(
                SlotCommit::<_, MockBlob, Vec<u8>>::new(MockBlock::default()),
                b"state-root",
            )
            .unwrap();

        assert_eq!(rx.blocking_recv().unwrap(), 0);
    }

    fn create_ledger(path: &std::path::Path) -> LedgerDb {
        let db = LedgerDb::get_rockbound_options()
            .default_setup_db_in_path(path)
            .unwrap();
        let cache_container = Arc::new(RwLock::new(CacheContainer::new(
            db,
            Arc::new(RwLock::new(Default::default())).into(),
        )));
        let cache_db = CacheDb::new(0, cache_container.into());
        LedgerDb::with_cache_db(cache_db).unwrap()
    }

    #[tokio::test]
    async fn test_save_aggregated_proof() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ledger_db = create_ledger(temp_dir.path());
        let _rx = ledger_db.proof_subscriptions.subscribe();

        let proof_from_db = ledger_db.get_latest_aggregated_proof().await.unwrap();
        assert_eq!(None, proof_from_db);

        for i in 0..10 {
            let public_data = AggregatedProofPublicData {
                validity_conditions: vec![],
                initial_slot_number: i as u64,
                final_slot_number: i as u64,
                genesis_state_root: vec![1],
                initial_state_root: vec![i],
                final_state_root: vec![i + 1],
                initial_slot_hash: vec![i + 2],
                final_slot_hash: vec![i + 3],
                code_commitment: CodeCommitment::default(),
            };

            let raw_aggregated_proof = MockZkvm::create_serialized_proof(true, public_data.clone());

            let agg_proof = AggregatedProof::new(
                SerializedAggregatedProof {
                    raw_aggregated_proof,
                },
                public_data.clone(),
            );

            ledger_db
                .save_finalized_aggregated_proof(agg_proof)
                .unwrap();

            let proof_from_db = ledger_db
                .get_latest_aggregated_proof()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&public_data, proof_from_db.proof.public_data());
        }
    }
}
