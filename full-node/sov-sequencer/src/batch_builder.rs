//! Concrete implementation(s) of [`BatchBuilder`].

use anyhow::{bail, Context as ErrorContext};
use borsh::BorshDeserialize;
use sov_db::sequencer_db::{MempoolTx, SequencerDB};
use sov_modules_api::digest::Digest;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::tx_verifier::TransactionAndRawHash;
use sov_modules_api::{CryptoSpec, Gas, GasArray, Spec, StateCheckpoint};
use sov_modules_stf_blueprint::{apply_tx, ExecutionMode, Runtime, TxEffect};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::services::batch_builder::{BatchBuilder, TxWithHash};
use tokio::sync::watch;

use crate::TxHash;

/// BatchBuilder that creates batches of transactions in the order they were submitted
/// Only transactions that were successfully dispatched are included.
pub struct FiFoStrictBatchBuilder<S: Spec, Da: DaSpec, R: Runtime<S, Da>> {
    mempool_max_txs_count: usize,
    runtime: R,
    max_batch_size_bytes: usize,
    current_storage: watch::Receiver<S::Storage>,
    sequencer: Da::Address,
    sequencer_db: SequencerDB,
}

impl<S, Da, R> FiFoStrictBatchBuilder<S, Da, R>
where
    S: Spec,
    Da: DaSpec,
    R: Runtime<S, Da>,
{
    /// [`BatchBuilder`] constructor.
    pub fn new(
        max_batch_size_bytes: usize,
        mempool_max_txs_count: usize,
        runtime: R,
        current_storage: watch::Receiver<<S as Spec>::Storage>,
        sequencer: Da::Address,
        sequencer_db: SequencerDB,
    ) -> Self {
        Self {
            mempool_max_txs_count,
            max_batch_size_bytes,
            runtime,
            current_storage,
            sequencer,
            sequencer_db,
        }
    }
}

impl<S, Da, R> BatchBuilder for FiFoStrictBatchBuilder<S, Da, R>
where
    S: Spec,
    Da: DaSpec,
    R: Runtime<S, Da>,
{
    /// Attempt to add transaction to the mempool.
    ///
    /// The transaction is discarded if:
    /// - mempool is full
    /// - transaction is invalid (deserialization, verification or decoding of the runtime message failed)
    fn accept_tx(&mut self, raw: Vec<u8>) -> anyhow::Result<TxHash> {
        tracing::trace!(raw_tx = hex::encode(&raw), "`accept_tx` has been called");
        if self.sequencer_db.txs_count() >= self.mempool_max_txs_count {
            bail!(
                "Mempool is full: transactions_count={}",
                self.sequencer_db.txs_count()
            )
        }

        if raw.len() > self.max_batch_size_bytes {
            bail!(
                "Transaction is too big. Max allowed size: {}, submitted size: {}",
                self.max_batch_size_bytes,
                raw.len()
            )
        }

        // Deserialize
        let tx = Transaction::<S>::deserialize(&mut raw.as_slice())
            .context("Failed to deserialize transaction")?;

        // Verify
        tx.verify().context("Failed to verify transaction")?;

        // Make sure the runtime message is valid
        R::decode_call(tx.runtime_msg())
            .map_err(anyhow::Error::new)
            .context("Failed to decode message in transaction")?;

        let hash = calculate_hash::<S>(&raw);
        tracing::debug!(
            raw_tx = hex::encode(&raw),
            hash = hex::encode(hash),
            "Adding a transaction to the mempool"
        );

        let tx = MempoolTx {
            tx_bytes: raw,
            runtime_msg: tx.runtime_msg().to_owned(),
            hash,
        };

        self.sequencer_db.push(tx)?;
        tracing::debug!(
            hash = hex::encode(hash),
            "Transaction has been added to the mempool"
        );
        Ok(hash)
    }

    fn contains(&self, hash: &TxHash) -> anyhow::Result<bool> {
        self.sequencer_db.contains(hash)
    }

    /// Builds a new batch of valid transactions in order they were added to mempool
    /// Only transactions, which are dispatched successfully are included in the batch
    fn get_next_blob(&mut self, height: u64) -> anyhow::Result<Vec<TxWithHash>> {
        tracing::debug!("get_next_blob has been called");
        let mut state_checkpoint = StateCheckpoint::new(self.current_storage.borrow().clone());

        let mut txs = Vec::new();
        let mut current_batch_size = 0;

        // TODO: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/224
        //     Use Kernel Hooks to get correct gas price
        // K: KernelSlotHooks<C, Da>>
        // let gas_price = self.kernel.begin_slot_hook(
        //     slot_header,
        //     validity_condition,
        //     pre_state_root,
        //     state_checkpoint,
        // );

        let gas_price = <S::Gas as Gas>::Price::ZEROED;
        let mut reward = 0;

        let count_before = self.sequencer_db.txs_count();
        tracing::debug!(
            txs_count = count_before,
            "Going to build batch from transactions in mempool"
        );
        while let Some(pooled) = self.sequencer_db.pop()? {
            // To fill a batch as big as possible, we only check if valid
            // tx can fit in the batch.
            let tx_len = pooled.tx_bytes.len();
            if current_batch_size + tx_len > self.max_batch_size_bytes {
                self.sequencer_db.reinsert(pooled)?;
                break;
            }

            // expect(): The transaction was accepted into the pool,
            // so we know that the runtime message is valid.
            let msg = R::decode_call(pooled.runtime_msg.as_slice())
                .expect("Undecodable transaction has been accepted into the pool");

            // Execute
            {
                let tx = Transaction::<S>::deserialize(&mut pooled.tx_bytes.as_slice())
                    .context("Failed to deserialize transaction")?;
                let tx_and_raw_hash = TransactionAndRawHash {
                    tx,
                    raw_tx_hash: pooled.hash,
                };
                let (after_state_checkpoint, tx_receipt) = apply_tx(
                    &self.runtime,
                    &tx_and_raw_hash,
                    msg,
                    state_checkpoint,
                    &self.sequencer,
                    &mut reward,
                    ExecutionMode::Speculative,
                    &gas_price,
                    height,
                );
                state_checkpoint = after_state_checkpoint;

                match tx_receipt.receipt {
                    TxEffect::Successful => {
                        tracing::info!(
                            hash = hex::encode(pooled.hash),
                            "Transaction has been included in the batch",
                        );

                        // Update size of current batch
                        current_batch_size += tx_len;

                        txs.push(TxWithHash {
                            raw_tx: pooled.tx_bytes,
                            hash: pooled.hash,
                        });
                    }
                    TxEffect::InsufficientBaseGas | TxEffect::Reverted | TxEffect::Duplicate => {
                        tracing::warn!(
                            ?tx_receipt,
                            tx = hex::encode(&pooled.tx_bytes),
                            hash = hex::encode(pooled.hash),
                            "Error during transaction dispatch"
                        );
                        continue;
                    }
                }
            }
        }

        if txs.is_empty() {
            bail!(
                "No valid transactions are available out of {} were in the pool",
                count_before
            );
        }

        tracing::info!(
            txs_count = txs.len(),
            "Batch of transactions has been built"
        );

        Ok(txs)
    }
}

fn calculate_hash<S: Spec>(tx_raw: &[u8]) -> TxHash {
    <S::CryptoSpec as CryptoSpec>::Hasher::digest(tx_raw).into()
}

#[cfg(test)]
mod tests {
    use borsh::BorshSerialize;
    use rand::Rng;
    use sov_mock_da::{MockAddress, MockDaSpec};
    use sov_modules_api::transaction::Transaction;
    use sov_modules_api::{EncodeCall, Genesis, PrivateKey, PublicKey, WorkingSet};
    use sov_prover_storage_manager::new_orphan_storage;
    use sov_rollup_interface::services::batch_builder::BatchBuilder;
    use sov_state::Storage;
    use sov_test_utils::runtime::{create_genesis_config, TestRuntime};
    use sov_test_utils::{TestPrivateKey, TestPublicKey, TestSpec};
    use sov_value_setter::{CallMessage, ValueSetter};
    use tempfile::TempDir;

    use super::*;

    const MAX_TX_POOL_SIZE: usize = 20;
    const DEFAULT_SEQUENCER_ADDRESS: MockAddress = MockAddress::new([0u8; 32]);

    type S = TestSpec;

    fn generate_random_valid_tx() -> Vec<u8> {
        let private_key = TestPrivateKey::generate();
        let mut rng = rand::thread_rng();
        let value: u32 = rng.gen();
        generate_valid_tx(&private_key, value)
    }

    fn generate_valid_tx(private_key: &TestPrivateKey, value: u32) -> Vec<u8> {
        let msg = CallMessage::SetValue(value);
        let msg = <TestRuntime<S, MockDaSpec> as EncodeCall<ValueSetter<S>>>::encode_call(msg);
        let chain_id = 0;
        let gas_tip = 0;
        let gas_limit = 0;
        let max_gas_price = None;
        let nonce = 1;

        Transaction::<TestSpec>::new_signed_tx(
            private_key,
            msg,
            chain_id,
            gas_tip,
            gas_limit,
            max_gas_price,
            nonce,
        )
        .try_to_vec()
        .unwrap()
    }

    fn generate_random_bytes() -> Vec<u8> {
        let mut rng = rand::thread_rng();

        let length = rng.gen_range(1..=512);

        (0..length).map(|_| rng.gen()).collect()
    }

    fn generate_signed_tx_with_invalid_payload(private_key: &TestPrivateKey) -> Vec<u8> {
        let msg = generate_random_bytes();
        let chain_id = 0;
        let gas_tip = 0;
        let gas_limit = 0;
        let max_gas_price = None;
        let nonce = 1;

        Transaction::<TestSpec>::new_signed_tx(
            private_key,
            msg,
            chain_id,
            gas_tip,
            gas_limit,
            max_gas_price,
            nonce,
        )
        .try_to_vec()
        .unwrap()
    }

    fn create_batch_builder(
        batch_size_bytes: usize,
        tmpdir: &TempDir,
        sequencer_address: MockAddress,
    ) -> FiFoStrictBatchBuilder<S, MockDaSpec, TestRuntime<S, MockDaSpec>> {
        let state_path = tmpdir.path().join("state");
        let sequencer_db_path = tmpdir.path().join("mempool");
        let storage = watch::Sender::new(new_orphan_storage(state_path).unwrap()).subscribe();
        let sequencer_db = SequencerDB::new(sequencer_db_path).unwrap();
        FiFoStrictBatchBuilder::new(
            batch_size_bytes,
            MAX_TX_POOL_SIZE,
            TestRuntime::<S, MockDaSpec>::default(),
            storage.clone(),
            sequencer_address,
            sequencer_db,
        )
    }

    fn setup_runtime(
        batch_builder: &mut FiFoStrictBatchBuilder<S, MockDaSpec, TestRuntime<S, MockDaSpec>>,
        admin: Option<TestPublicKey>,
        admin_da_address: MockAddress,
    ) {
        let runtime = TestRuntime::<S, MockDaSpec>::default();
        let storage = batch_builder.current_storage.borrow().clone();
        let mut working_set = WorkingSet::new(storage.clone());

        let admin = admin.unwrap_or_else(|| {
            let admin_private_key = TestPrivateKey::generate();
            admin_private_key.pub_key()
        });
        let admin = admin.to_address();
        let config = create_genesis_config(
            admin,
            admin_da_address,
            100,
            "BatchBuilderTestToken".to_string(),
            10,
            100_000,
        );
        runtime.genesis(&config, &mut working_set).unwrap();
        let (log, witness) = working_set.checkpoint().0.freeze();
        storage.validate_and_commit(log, &witness).unwrap();
    }

    mod accept_tx {
        use super::*;

        #[test]
        fn accept_valid_tx() {
            let tx = generate_random_valid_tx();

            let tmpdir = tempfile::tempdir().unwrap();
            let mut batch_builder =
                create_batch_builder(tx.len(), &tmpdir, DEFAULT_SEQUENCER_ADDRESS);

            batch_builder.accept_tx(tx).unwrap();
        }

        #[test]
        fn reject_tx_too_big() {
            let tx = generate_random_valid_tx();
            let tx_size = tx.len();
            let batch_size = tx.len().saturating_sub(1);

            let tmpdir = tempfile::tempdir().unwrap();
            let mut batch_builder =
                create_batch_builder(batch_size, &tmpdir, DEFAULT_SEQUENCER_ADDRESS);

            let accept_result = batch_builder.accept_tx(tx);
            assert!(accept_result.is_err());
            assert_eq!(
                format!("Transaction is too big. Max allowed size: {batch_size}, submitted size: {tx_size}"),
                accept_result.unwrap_err().to_string()
            );
        }

        #[test]
        fn reject_tx_on_full_mempool() {
            let tmpdir = tempfile::tempdir().unwrap();
            let mut batch_builder =
                create_batch_builder(usize::MAX, &tmpdir, DEFAULT_SEQUENCER_ADDRESS);

            for _ in 0..MAX_TX_POOL_SIZE {
                let tx = generate_random_valid_tx();
                batch_builder.accept_tx(tx).unwrap();
            }

            let tx = generate_random_valid_tx();
            let accept_result = batch_builder.accept_tx(tx);

            assert!(accept_result.is_err());
            let expected_error_message =
                format!("Mempool is full: transactions_count={}", MAX_TX_POOL_SIZE);
            assert_eq!(
                expected_error_message,
                accept_result.unwrap_err().to_string()
            );
        }

        #[test]
        fn reject_random_bytes_tx() {
            let tx = generate_random_bytes();

            let tmpdir = tempfile::tempdir().unwrap();
            let mut batch_builder =
                create_batch_builder(tx.len(), &tmpdir, DEFAULT_SEQUENCER_ADDRESS);

            let accept_result = batch_builder.accept_tx(tx);
            assert!(accept_result.is_err());
            assert!(accept_result
                .unwrap_err()
                .to_string()
                .starts_with("Failed to deserialize transaction"));
        }

        #[test]
        fn reject_signed_tx_with_invalid_payload() {
            let private_key = TestPrivateKey::generate();
            let tx = generate_signed_tx_with_invalid_payload(&private_key);

            let tmpdir = tempfile::tempdir().unwrap();
            let mut batch_builder =
                create_batch_builder(tx.len(), &tmpdir, DEFAULT_SEQUENCER_ADDRESS);

            let accept_result = batch_builder.accept_tx(tx);
            assert!(accept_result.is_err());
            assert!(accept_result
                .unwrap_err()
                .to_string()
                .starts_with("Failed to decode message"));
        }

        #[test]
        fn zero_sized_mempool_cant_accept_tx() {
            let tx = generate_random_valid_tx();

            let tmpdir = tempfile::tempdir().unwrap();
            let mut batch_builder =
                create_batch_builder(tx.len(), &tmpdir, DEFAULT_SEQUENCER_ADDRESS);
            batch_builder.mempool_max_txs_count = 0;

            let accept_result = batch_builder.accept_tx(tx);
            assert!(accept_result.is_err());
            assert_eq!(
                "Mempool is full: transactions_count=0",
                accept_result.unwrap_err().to_string()
            );
        }
    }

    mod build_batch {
        use super::*;

        #[test]
        fn error_on_empty_mempool() {
            let tmpdir = tempfile::tempdir().unwrap();
            let mut batch_builder = create_batch_builder(10, &tmpdir, DEFAULT_SEQUENCER_ADDRESS);
            setup_runtime(&mut batch_builder, None, DEFAULT_SEQUENCER_ADDRESS);

            let build_result = batch_builder.get_next_blob(1);
            assert!(build_result.is_err());
            assert_eq!(
                "No valid transactions are available out of 0 were in the pool",
                build_result.unwrap_err().to_string()
            );
        }

        #[test]
        #[should_panic = "Sequencer is no longer registered by the time of context resolution. This is a bug"]
        fn build_batch_invalidates_everything_on_missed_genesis() {
            let value_setter_admin = TestPrivateKey::generate();
            let txs = [
                // Should be included: 113 bytes
                generate_valid_tx(&value_setter_admin, 1),
                generate_valid_tx(&value_setter_admin, 2),
            ];

            let tmpdir = tempfile::tempdir().unwrap();
            let batch_size = txs[0].len() * 3 + 1;
            let mut batch_builder =
                create_batch_builder(batch_size, &tmpdir, DEFAULT_SEQUENCER_ADDRESS);
            // Skipping runtime setup

            for tx in &txs {
                batch_builder.accept_tx(tx.clone()).unwrap();
            }

            assert_eq!(txs.len(), batch_builder.sequencer_db.txs_count());

            let _ = batch_builder.get_next_blob(1);
        }

        #[test]
        fn builds_batch_skipping_invalid_txs() {
            let value_setter_admin = TestPrivateKey::generate();
            let txs = [
                // Should be included: 113 bytes
                generate_valid_tx(&value_setter_admin, 1),
                // Should be rejected, not admin
                generate_random_valid_tx(),
                // Should be included: 113 bytes
                generate_valid_tx(&value_setter_admin, 2),
                // Should be skipped, more than batch size
                generate_valid_tx(&value_setter_admin, 3),
            ];

            let tmpdir = tempfile::tempdir().unwrap();
            let batch_size = txs[0].len() + txs[2].len() + 1;
            let mut batch_builder =
                create_batch_builder(batch_size, &tmpdir, DEFAULT_SEQUENCER_ADDRESS);
            setup_runtime(
                &mut batch_builder,
                Some(value_setter_admin.pub_key()),
                DEFAULT_SEQUENCER_ADDRESS,
            );

            for tx in &txs {
                batch_builder.accept_tx(tx.clone()).unwrap();
            }

            assert_eq!(txs.len(), batch_builder.sequencer_db.txs_count());

            let build_result = batch_builder.get_next_blob(1);
            let blob = build_result
                .unwrap()
                .iter()
                // We discard hashes for the sake of comparison
                .map(|t| t.raw_tx.clone())
                .collect::<Vec<_>>();
            assert_eq!(2, blob.len());
            assert!(blob.contains(&txs[0]));
            assert!(blob.contains(&txs[2]));
            assert!(!blob.contains(&txs[3]));
            assert_eq!(1, batch_builder.sequencer_db.txs_count());
        }
    }
}
