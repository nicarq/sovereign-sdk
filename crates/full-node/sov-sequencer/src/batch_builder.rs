//! Concrete implementation(s) of [`BatchBuilder`].

use anyhow::bail;
use async_trait::async_trait;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sov_modules_api::capabilities::{
    KernelSlotHooks, SequencerAuthorization, TransactionAuthenticator,
};
use sov_modules_api::runtime::capabilities::Kernel;
use sov_modules_api::transaction::SequencerReward;
use sov_modules_api::{
    ExecutionContext, FullyBakedTx, Gas, GasArray, RawTx, Spec, StateCheckpoint, VersionReader,
};
use sov_modules_stf_blueprint::{
    process_tx, ApplyTxResult, Runtime, TransactionReceipt, TxEffect, TxProcessingError,
    TxProcessingErrorReason,
};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::node::batch_builder::{AcceptTxError, BatchBuilder, TxWithHash};
use tokio::sync::watch;
use tracing::error;

use crate::db::{MempoolTx, SequencerDb};
use crate::mempool::{FairMempool, MempoolCursor};
use crate::tx_status::TxStatusManager;
use crate::TxHash;

/// Configuration for [`FairBatchBuilder`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct FairBatchBuilderConfig<Da: DaSpec> {
    /// Maximum number of transactions in mempool. Once this limit is reached,
    /// the batch builder will evict older transactions.
    pub mempool_max_txs_count: Option<usize>,
    /// Maximum size of a batch. The batch builder will not build batches larger
    /// than this size.
    pub max_batch_size_bytes: Option<usize>,
    /// DA address of the sequencer.
    pub sequencer_address: Da::Address,
}

/// A [`BatchBuilder`] that creates batches of transactions in a way that's
/// reasonably "fair" to everybody.
///
/// Transactions are included in batches by following a largest-first,
/// least-recent-first priority. Only transactions that were successfully
/// dispatched are included.
pub struct FairBatchBuilder<S: Spec, Da: DaSpec, R: Runtime<S, Da>, K> {
    runtime: R,
    kernel: K,
    mempool: FairMempool<Da>,
    max_batch_size_bytes: usize,
    current_storage: watch::Receiver<S::Storage>,
    sequencer: Da::Address,
}

impl<S, Da, R, K> FairBatchBuilder<S, Da, R, K>
where
    S: Spec,
    Da: DaSpec,
    R: Runtime<S, Da>,
{
    const DEFAULT_MEMPOOL_MAX_TXS_COUNT: usize = 100;
    const DEFAULT_MAX_BATCH_SIZE_BYTES: usize = 1024 * 1024;

    /// [`BatchBuilder`] constructor.
    pub fn new(
        runtime: R,
        kernel: K,
        tx_status_manager: TxStatusManager<Da>,
        current_storage: watch::Receiver<<S as Spec>::Storage>,
        sequencer_db: SequencerDb,
        config: FairBatchBuilderConfig<Da>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            mempool: FairMempool::new(
                sequencer_db,
                tx_status_manager,
                config
                    .mempool_max_txs_count
                    .unwrap_or(Self::DEFAULT_MEMPOOL_MAX_TXS_COUNT),
            )?,
            max_batch_size_bytes: config
                .max_batch_size_bytes
                .unwrap_or(Self::DEFAULT_MAX_BATCH_SIZE_BYTES),
            runtime,
            kernel,
            current_storage,
            sequencer: config.sequencer_address,
        })
    }

    /// Returns [`None`] if the transaction does not fit inside the batch.
    fn try_add_tx_to_batch(
        &self,
        mempool_tx: &MempoolTx,
        mut ctx: BatchConstructionContext<S>,
    ) -> (
        BatchConstructionContext<S>,
        Result<Option<TransactionReceipt<S>>, TxProcessingErrorReason>,
    ) {
        // To fill a batch as big as possible, we only check if valid
        // tx can fit in the batch.
        let tx_len = mempool_tx.tx_bytes.len();
        if ctx.current_batch_size_in_bytes + tx_len > self.max_batch_size_bytes {
            return (ctx, Ok(None));
        }

        let tx_scratchpad = ctx.state_checkpoint.to_tx_scratchpad();
        let res = process_tx(
            &self.runtime,
            &FullyBakedTx {
                data: mempool_tx.tx_bytes.clone(),
            },
            &self.sequencer,
            &ctx.gas_price,
            ctx.visible_height,
            tx_scratchpad,
            ExecutionContext::Sequencer,
        );

        match res {
            Err(TxProcessingError {
                tx_scratchpad,
                reason,
            }) => {
                // ...and immediately store the new `StateCheckpoint`.
                ctx.state_checkpoint = tx_scratchpad.revert();

                (ctx, Err(reason))
            }
            Ok(ApplyTxResult {
                tx_scratchpad,
                receipt,
                sequencer_reward,
            }) => {
                // ...and immediately store the new `StateCheckpoint`.
                ctx.state_checkpoint = tx_scratchpad.commit();
                ctx.reward.accumulate(sequencer_reward);

                (ctx, Ok(Some(receipt)))
            }
        }
    }

    fn mempool_cursor(&self, ctx: &BatchConstructionContext<S>) -> MempoolCursor {
        MempoolCursor::new(
            self.max_batch_size_bytes
                .checked_sub(ctx.current_batch_size_in_bytes)
                .unwrap(),
        )
    }
}

#[async_trait]
impl<S, Da, R, K> BatchBuilder for FairBatchBuilder<S, Da, R, K>
where
    S: Spec,
    Da: DaSpec,
    R: Runtime<S, Da> + TransactionAuthenticator<S> + 'static,
    K: Kernel<S::Storage> + KernelSlotHooks<S, Da> + 'static,
{
    type Config = FairBatchBuilderConfig<Da>;

    /// Attempt to add transaction to the mempool.
    ///
    /// The transaction is discarded if:
    /// - mempool is full
    /// - transaction is invalid (deserialization, verification or decoding of the runtime message failed)
    async fn accept_tx(&mut self, raw: Vec<u8>) -> Result<TxWithHash, AcceptTxError> {
        tracing::trace!(raw_tx = hex::encode(&raw), "`accept_tx` has been called");
        let authenticated = R::add_standard_auth(RawTx { data: raw });
        let raw = borsh::to_vec(&authenticated).map_err(|e| AcceptTxError {
            http_status: StatusCode::BAD_REQUEST.as_u16(),
            title: "Failed to encode transaction".to_string(),
            details: format!("{:?}", e),
        })?;

        if raw.len() > self.max_batch_size_bytes {
            return Err(AcceptTxError {
                http_status: StatusCode::BAD_REQUEST.as_u16(),
                title: "Transaction is too big".to_string(),
                details: format!(
                    "Max allowed size: {}, submitted size: {}",
                    self.max_batch_size_bytes,
                    raw.len(),
                ),
            });
        }

        let storage: S::Storage = self.current_storage.borrow().clone();
        let state_checkpoint = StateCheckpoint::new(storage, &self.kernel);
        let tx_scratchpad = state_checkpoint.to_tx_scratchpad();

        let runtime = R::default();
        let mut pre_exec_ws = match runtime.sequencer_authorization().authorize_sequencer(
            &self.sequencer,
            &<S::Gas as Gas>::Price::ZEROED,
            tx_scratchpad,
        ) {
            Ok(res) => res.1,
            Err(error) => {
                error!(
                    ?error,
                    "Sequencer authorization error; you may not have enough stake!"
                );
                return Err(AcceptTxError {
                    http_status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                    title: "Sequencer authorization error".to_string(),
                    details: format!("{:?}", error),
                });
            }
        };

        let auth_result = runtime
            .authenticate(&authenticated, &mut pre_exec_ws)
            .map_err(|e| AcceptTxError {
                http_status: StatusCode::BAD_REQUEST.as_u16(),
                title: "The transaction is invalid".to_string(),
                details: format!("{:?}", e),
            })?;

        let hash = auth_result.0.raw_tx_hash;
        tracing::debug!(
            raw_tx = hex::encode(&raw),
            %hash,
            "Adding a transaction to the mempool"
        );

        self.mempool
            .add_new_tx(hash, raw.clone())
            .map_err(|err| AcceptTxError {
                http_status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                title: "Failed to submit transaction".to_string(),
                details: format!("{:?}", err),
            })?;
        tracing::trace!(
            %hash,
            "Transaction has been added to the mempool"
        );

        Ok(TxWithHash { hash, raw_tx: raw })
    }

    async fn contains(&self, hash: &TxHash) -> anyhow::Result<bool> {
        Ok(self.mempool.contains(hash))
    }

    /// Builds a new batch of valid transactions in order they were added to mempool.
    /// Only transactions which are dispatched successfully are included in the batch.
    async fn get_next_blob(&mut self, _height: u64) -> anyhow::Result<Vec<TxWithHash>> {
        tracing::debug!("get_next_blob has been called");

        let mut state_checkpoint =
            StateCheckpoint::new(self.current_storage.borrow().clone(), &self.kernel);

        let gas_price = self.kernel.base_fee_per_gas(&mut state_checkpoint);

        let visible_height = state_checkpoint.rollup_height_to_access();

        let mut ctx = BatchConstructionContext {
            visible_height,
            reward: SequencerReward::ZERO,
            gas_price,
            state_checkpoint,
            current_batch_size_in_bytes: 0,
        };

        let mut txs = Vec::new();

        let count_before = self.mempool.len();
        tracing::debug!(
            txs_count = count_before,
            "Going to build batch from transactions in mempool"
        );

        let mut cursor = self.mempool_cursor(&ctx);

        while let Some(mempool_tx) = self.mempool.next(&mut cursor) {
            let tx_receipt = match self.try_add_tx_to_batch(&mempool_tx, ctx) {
                (c, Ok(txr)) => {
                    ctx = c;
                    txr
                }
                (c, Err(TxProcessingErrorReason::Nonce { .. })) => {
                    tracing::info!(
                        hash = %mempool_tx.hash,
                        "Transaction processing error due to nonce; ignoring tx",
                    );
                    ctx = c;
                    continue;
                }
                (_c, Err(reason)) => {
                    bail!("An non-recoverable error occurred when trying to add the tx to the batch: {reason}")
                }
            };

            match tx_receipt.map(|r| r.receipt) {
                Some(TxEffect::Successful(_)) => {
                    tracing::info!(
                        hash = %mempool_tx.hash,
                        "Transaction has been included in the batch",
                    );

                    let tx_len = mempool_tx.tx_bytes.len();
                    ctx.current_batch_size_in_bytes += tx_len;

                    txs.push(TxWithHash {
                        raw_tx: mempool_tx.tx_bytes.clone(),
                        hash: mempool_tx.hash,
                    });

                    // Update the cursor to reflect the new amount of available
                    // space inside the batch.
                    cursor = cursor.max(self.mempool_cursor(&ctx));
                }
                Some(tx_receipt) => {
                    // Failed transaction; ignore and process the next one.
                    tracing::warn!(
                        ?tx_receipt,
                        tx = hex::encode(&mempool_tx.tx_bytes),
                        hash = %mempool_tx.hash,
                        "Error during transaction dispatch"
                    );
                    continue;
                }
                None => {
                    // We couldn't find any transaction that fits in the
                    // remaining space inside the batch; we're done.
                    break;
                }
            }
        }

        self.mempool
            .remove_atomically(txs.iter().map(|tx| tx.hash).collect::<Vec<_>>().as_slice())?;

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

struct BatchConstructionContext<S: Spec> {
    state_checkpoint: StateCheckpoint<S::Storage>,
    visible_height: u64,
    reward: SequencerReward,
    gas_price: <S::Gas as Gas>::Price,
    current_batch_size_in_bytes: usize,
}

#[cfg(test)]
mod tests {
    use rand::Rng;
    use sov_kernels::basic::BasicKernel;
    use sov_mock_da::{MockAddress, MockDaSpec};
    use sov_modules_api::macros::config_value;
    use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
    use sov_modules_api::{EncodeCall, PrivateKey, StateTransitionFunction};
    use sov_state::ProverStorage;
    use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
    use sov_test_utils::runtime::{GenesisConfig, TestOptimisticRuntime, ValueSetterConfig};
    use sov_test_utils::storage::{new_finalized_storage, SimpleStorageManager};
    use sov_test_utils::{
        TestPrivateKey, TestSequencer, TestSpec, TestStorageSpec as StorageSpec, TestUser,
        TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE,
    };
    use sov_value_setter::{CallMessage, ValueSetter};
    use tempfile::TempDir;

    use super::*;

    const MAX_TX_POOL_SIZE: usize = 20;
    const DEFAULT_SEQUENCER_DA_ADDRESS: MockAddress = MockAddress::new([0u8; 32]);

    type S = TestSpec;

    type BatchBuilder = FairBatchBuilder<
        S,
        MockDaSpec,
        TestOptimisticRuntime<S, MockDaSpec>,
        BasicKernel<S, MockDaSpec>,
    >;

    fn generate_random_valid_tx(nonce: u64) -> RawTx {
        let private_key = TestPrivateKey::generate();
        let mut rng = rand::thread_rng();
        let value: u32 = rng.gen();
        generate_valid_tx(&private_key, nonce, value)
    }

    fn generate_valid_tx(private_key: &TestPrivateKey, nonce: u64, value: u32) -> RawTx {
        let msg = CallMessage::SetValue(value);
        let msg =
            <TestOptimisticRuntime<_, MockDaSpec> as EncodeCall<ValueSetter<S>>>::encode_call(msg);
        let chain_id = config_value!("CHAIN_ID");
        let max_priority_fee_bips = TEST_DEFAULT_MAX_PRIORITY_FEE;
        let max_fee = TEST_DEFAULT_MAX_FEE;
        let gas_limit = None;

        let tx = borsh::to_vec(&Transaction::<S>::new_signed_tx(
            private_key,
            UnsignedTransaction::new(
                msg,
                chain_id,
                max_priority_fee_bips,
                max_fee,
                nonce,
                gas_limit,
            ),
        ))
        .unwrap();

        RawTx::new(tx)
    }

    fn generate_random_bytes() -> Vec<u8> {
        let mut rng = rand::thread_rng();

        let length = rng.gen_range(1..=512);

        (0..length).map(|_| rng.gen()).collect()
    }

    fn generate_signed_tx_with_invalid_payload(private_key: &TestPrivateKey, nonce: u64) -> RawTx {
        let msg = generate_random_bytes();
        let chain_id = config_value!("CHAIN_ID");
        let max_priority_fee_bips = TEST_DEFAULT_MAX_PRIORITY_FEE;
        let max_fee = TEST_DEFAULT_MAX_FEE;
        let gas_limit = None;

        let tx = borsh::to_vec(&Transaction::<S>::new_signed_tx(
            private_key,
            UnsignedTransaction::new(
                msg,
                chain_id,
                max_priority_fee_bips,
                max_fee,
                nonce,
                gas_limit,
            ),
        ))
        .unwrap();

        RawTx::new(tx)
    }

    fn create_batch_builder(
        batch_size_bytes: usize,
        tmpdir: &TempDir,
        initial_storage: Option<ProverStorage<StorageSpec>>,
        sequencer_address: MockAddress,
    ) -> (BatchBuilder, watch::Sender<ProverStorage<StorageSpec>>) {
        let sequencer_db_path = tmpdir.path().join("mempool");
        let storage = initial_storage.unwrap_or_else(|| {
            let state_path = tmpdir.path().join("state");
            new_finalized_storage(state_path)
        });
        let storage_sender = watch::Sender::new(storage);
        let storage = storage_sender.subscribe();
        let sequencer_db = SequencerDb::new(sequencer_db_path).unwrap();
        let tx_status_manager = TxStatusManager::default();

        let config = FairBatchBuilderConfig {
            mempool_max_txs_count: Some(MAX_TX_POOL_SIZE),
            max_batch_size_bytes: Some(batch_size_bytes),
            sequencer_address,
        };

        let batch_builder = BatchBuilder::new(
            TestOptimisticRuntime::<S, MockDaSpec>::default(),
            BasicKernel::default(),
            tx_status_manager,
            storage,
            sequencer_db,
            config,
        )
        .unwrap();

        (batch_builder, storage_sender)
    }

    /// Struct returned by [`setup_runtime`] which contains all the data needed to run the tests.
    pub struct SetupOutput {
        storage: ProverStorage<StorageSpec>,
        additional_accounts: Vec<TestUser<S>>,
        admin: TestUser<S>,
        sequencer: TestSequencer<S, MockDaSpec>,
    }

    fn setup_runtime(
        storage_manager: &mut SimpleStorageManager<StorageSpec>,
        num_additional_accounts: usize,
    ) -> SetupOutput {
        let runtime = TestOptimisticRuntime::<S, MockDaSpec>::default();
        let stf = sov_modules_stf_blueprint::StfBlueprint::<
            S,
            MockDaSpec,
            TestOptimisticRuntime<S, MockDaSpec>,
            BasicKernel<S, MockDaSpec>,
        >::with_runtime(runtime);

        let genesis_config = HighLevelOptimisticGenesisConfig::generate()
            .add_accounts_with_default_balance(num_additional_accounts + 1);

        let admin = genesis_config.additional_accounts[0].clone();

        let value_setter_config = ValueSetterConfig {
            admin: admin.address(),
        };

        let additional_accounts = genesis_config.additional_accounts[1..].to_vec().clone();
        let sequencer = genesis_config.initial_sequencer.clone();
        let config = GenesisConfig::from_minimal_config(genesis_config.into(), value_setter_config);
        let stf_state = storage_manager.create_storage();
        let (_, change_set) = stf.init_chain(stf_state, config.into_genesis_params());

        storage_manager.commit(change_set);

        SetupOutput {
            storage: storage_manager.create_storage(),
            additional_accounts,
            admin,
            sequencer,
        }
    }

    mod accept_tx {
        use sov_rollup_interface::node::batch_builder::BatchBuilder;

        use super::*;

        #[tokio::test]
        async fn accept_valid_tx() {
            let tx = generate_random_valid_tx(0);

            let tmpdir = tempfile::tempdir().unwrap();
            let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
            let SetupOutput {
                storage, sequencer, ..
            } = setup_runtime(&mut storage_manager, 0);

            let sequencer_da_address = sequencer.da_address;

            let authenticated_tx =
                TestOptimisticRuntime::<S, MockDaSpec>::encode_with_standard_auth(tx.clone());

            let (mut batch_builder, _storage) = create_batch_builder(
                authenticated_tx.data.len(),
                &tmpdir,
                Some(storage),
                sequencer_da_address,
            );

            batch_builder.accept_tx(tx.data).await.unwrap();
        }

        #[tokio::test]
        async fn reject_tx_too_big() {
            let tx = generate_random_valid_tx(0);
            let batch_size = tx.data.len().saturating_sub(1);

            let tmpdir = tempfile::tempdir().unwrap();
            let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
            let SetupOutput {
                storage, sequencer, ..
            } = setup_runtime(&mut storage_manager, 0);

            let sequencer_da_address = sequencer.da_address;

            let (mut batch_builder, _storage) =
                create_batch_builder(batch_size, &tmpdir, Some(storage), sequencer_da_address);

            let accept_result = batch_builder.accept_tx(tx.data).await;
            assert!(accept_result.is_err());
            assert_eq!(accept_result.unwrap_err().title, "Transaction is too big");
        }

        #[tokio::test]
        async fn new_tx_on_full_mempool_causes_evictions() {
            let tmpdir = tempfile::tempdir().unwrap();
            let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
            let SetupOutput {
                storage, sequencer, ..
            } = setup_runtime(&mut storage_manager, 0);

            let sequencer_da_address = sequencer.da_address;

            let (mut batch_builder, _storage) =
                create_batch_builder(usize::MAX, &tmpdir, Some(storage), sequencer_da_address);

            for i in 0..MAX_TX_POOL_SIZE {
                let tx = generate_random_valid_tx(i as u64);
                batch_builder.accept_tx(tx.data).await.unwrap();
            }

            assert_eq!(MAX_TX_POOL_SIZE, batch_builder.mempool.len());

            let tx = generate_random_valid_tx(MAX_TX_POOL_SIZE as u64);
            batch_builder.accept_tx(tx.data).await.unwrap();

            assert_eq!(MAX_TX_POOL_SIZE, batch_builder.mempool.len());
        }

        #[tokio::test]
        async fn reject_random_bytes_tx() {
            let tx = generate_random_bytes();

            let tmpdir = tempfile::tempdir().unwrap();
            let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
            let SetupOutput {
                storage, sequencer, ..
            } = setup_runtime(&mut storage_manager, 0);

            let sequencer_da_address = sequencer.da_address;

            let (mut batch_builder, _storage) =
                create_batch_builder(tx.len(), &tmpdir, Some(storage), sequencer_da_address);

            let accept_result = batch_builder.accept_tx(tx).await;
            assert!(accept_result.is_err());
        }

        #[tokio::test]
        async fn reject_signed_tx_with_invalid_payload() {
            let private_key = TestPrivateKey::generate();
            let tx = generate_signed_tx_with_invalid_payload(&private_key, 0);

            let tmpdir = tempfile::tempdir().unwrap();
            let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
            let SetupOutput {
                storage, sequencer, ..
            } = setup_runtime(&mut storage_manager, 0);

            let sequencer_da_address = sequencer.da_address;
            let authenticated_tx =
                &TestOptimisticRuntime::<S, MockDaSpec>::encode_with_standard_auth(tx.clone());

            let (mut batch_builder, _storage) = create_batch_builder(
                authenticated_tx.data.len(),
                &tmpdir,
                Some(storage),
                sequencer_da_address,
            );

            let accept_result = batch_builder.accept_tx(tx.data).await;
            assert!(accept_result.is_err());
            assert!(accept_result
                .unwrap_err()
                .details
                .contains("MessageDecodingFailed"));
        }
    }

    mod build_batch {
        use sov_rollup_interface::node::batch_builder::BatchBuilder;
        use sov_test_utils::storage::SimpleStorageManager;

        use super::*;

        #[tokio::test]
        async fn error_on_empty_mempool() {
            let tmpdir = tempfile::tempdir().unwrap();
            let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
            let SetupOutput {
                storage, sequencer, ..
            } = setup_runtime(&mut storage_manager, 0);

            let seq_da_address = sequencer.da_address;

            let (mut batch_builder, _storage) =
                create_batch_builder(10, &tmpdir, Some(storage), seq_da_address);

            let build_result = batch_builder.get_next_blob(1).await;
            assert!(build_result.is_err());
            assert_eq!(
                "No valid transactions are available out of 0 were in the pool",
                build_result.unwrap_err().to_string()
            );
        }

        #[tokio::test]
        async fn duplicate_txs_are_ignored() {
            let tmpdir = tempfile::tempdir().unwrap();
            let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
            let SetupOutput {
                storage,
                admin,
                sequencer,
                ..
            } = setup_runtime(&mut storage_manager, 0);

            let txs = [
                // Two identical txs...
                generate_valid_tx(&admin.private_key, 0, 1),
                generate_valid_tx(&admin.private_key, 0, 1),
            ];

            let (mut batch_builder, _storage) =
                create_batch_builder(usize::MAX, &tmpdir, Some(storage), sequencer.da_address);

            for tx in &txs {
                batch_builder.accept_tx(tx.clone().data).await.unwrap();
            }

            // The resulting batch should contain only one transaction (not two,
            // because we the second one is a duplicate!).
            assert_eq!(batch_builder.get_next_blob(1).await.unwrap().len(), 1);
        }

        #[tokio::test]
        async fn build_batch_invalidates_everything_on_missed_genesis() {
            let value_setter_admin = TestPrivateKey::generate();
            let txs = [
                // Should be included: 113 bytes
                generate_valid_tx(&value_setter_admin, 0, 1),
                generate_valid_tx(&value_setter_admin, 1, 2),
            ];

            let batch_size = txs[0].data.len() * 3 + 1;

            let tmpdir = tempfile::tempdir().unwrap();
            let (mut batch_builder, _storage) =
                create_batch_builder(batch_size, &tmpdir, None, DEFAULT_SEQUENCER_DA_ADDRESS);

            for tx in &txs {
                // We skipped genesis, so there is no registered sequencer. All
                // txs should be rejected immediately during authentication
                // checks.
                assert!(batch_builder.accept_tx(tx.data.clone()).await.is_err());
            }
        }

        #[tokio::test]
        async fn builds_batch_skipping_invalid_txs() {
            let tmpdir = tempfile::tempdir().unwrap();
            let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
            let SetupOutput {
                storage,
                sequencer,
                additional_accounts,
                admin,
            } = setup_runtime(&mut storage_manager, 1);

            let additional_account = additional_accounts[0].clone();

            let txs = [
                // Should be included
                generate_valid_tx(&admin.private_key, 0, 1),
                // Should be rejected, not admin
                generate_valid_tx(&additional_account.private_key, 0, 2),
                // Should be included
                generate_valid_tx(&admin.private_key, 1, 3),
                // Should be skipped, more than batch size
                generate_valid_tx(&admin.private_key, 2, 4),
            ];

            let authenticated_tx_0 = borsh::to_vec(
                &TestOptimisticRuntime::<S, MockDaSpec>::add_standard_auth(txs[0].clone()),
            )
            .unwrap();
            let authenticated_tx_2 = borsh::to_vec(
                &TestOptimisticRuntime::<S, MockDaSpec>::add_standard_auth(txs[2].clone()),
            )
            .unwrap();

            let batch_size = authenticated_tx_0.len() + authenticated_tx_2.len() + 1;
            let (mut batch_builder, _storage) =
                create_batch_builder(batch_size, &tmpdir, Some(storage), sequencer.da_address);

            assert!(
                txs.iter().all(|tx| tx.data.len() == txs[0].data.len()),
                "the test assumes all txs have equal length"
            );

            let mut raw_txs = Vec::new();
            for tx in &txs {
                let raw_tx = batch_builder
                    .accept_tx(tx.data.clone())
                    .await
                    .unwrap()
                    .raw_tx;
                raw_txs.push(raw_tx);
            }

            assert_eq!(txs.len(), batch_builder.mempool.len());

            let build_result = batch_builder.get_next_blob(1).await;
            let blob = build_result
                .unwrap()
                .iter()
                // We discard hashes for the sake of comparison
                .map(|t| t.raw_tx.clone())
                .collect::<Vec<_>>();
            assert_eq!(2, blob.len());
            assert!(blob.contains(&raw_txs[0]));
            assert!(!blob.contains(&raw_txs[1]));
            assert!(blob.contains(&raw_txs[2]));
            assert!(!blob.contains(&raw_txs[3]));
            assert_eq!(2, batch_builder.mempool.len());
        }
    }
}
