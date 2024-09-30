//! Standard, "vanilla" non-preferred sequencer implementation.

mod mempool;

use std::num::NonZero;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use schemars::JsonSchema;
use serde::Deserialize;
use sov_modules_api::capabilities::{
    AuthenticationError, AuthorizeSequencerError, HasCapabilities, KernelSlotHooks,
    KernelWithSlotMapping, SequencerAuthorization, TransactionAuthenticator,
};
use sov_modules_api::rest::{ApiState, StorageReceiver};
use sov_modules_api::runtime::capabilities::Kernel;
use sov_modules_api::transaction::SequencerReward;
use sov_modules_api::{
    AuthorizeTransactionError, Batch, ExecutionContext, FullyBakedTx, Gas, RawTx, Spec,
    StateCheckpoint, VersionReader,
};
use sov_modules_stf_blueprint::{
    process_tx, ApplyTxResult, TransactionReceipt, TxEffect, TxProcessingError,
};
use sov_rollup_interface::da::DaSpec;
use tokio::sync::watch;
use tracing::error;

use self::mempool::{Mempool, MempoolCursor};
use super::RtAwareBatchBuilderSpec;
use crate::batch_builders::{
    AcceptTxError, AcceptedTx, BatchBuilder, FreshlyBuiltBatch, TxWithHash,
};
use crate::db::SeqDbTx;
use crate::{TxHash, TxStatus, TxStatusManager};

/// Configuration for [`StdBatchBuilder`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct StdBatchBuilderConfig {
    /// Maximum number of transactions in mempool. Once this limit is reached,
    /// the batch builder will evict older transactions.
    pub mempool_max_txs_count: Option<NonZero<usize>>,
    /// Maximum size of a batch. The batch builder will not build batches larger
    /// than this size.
    pub max_batch_size_bytes: Option<usize>,
}

/// A [`BatchBuilder`] that creates batches of transactions in a way that's
/// reasonably "fair" to everybody.
///
/// Transactions are included in batches by following a largest-first,
/// least-recent-first priority. Only transactions that were successfully
/// dispatched are included.
pub struct StdBatchBuilder<Z: RtAwareBatchBuilderSpec, K> {
    runtime: Z::Rt,
    kernel: Arc<K>,
    txsm: TxStatusManager<Z::Da>,
    mempool: Mempool<Z::Da>,
    checkpoint: Option<StateCheckpoint<<Z::Spec as Spec>::Storage>>,
    checkpoint_sender: watch::Sender<StateCheckpoint<<Z::Spec as Spec>::Storage>>,
    api_state: ApiState<(), Z::Spec>,
    storage_recv: StorageReceiver<Z::Spec>,
    tx_hashes_of_last_batch: Vec<TxHash>,
    sequencer_address: <Z::Da as DaSpec>::Address,
    config: StdBatchBuilderConfig,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct TxInfo<BlobHash> {
    id: TxHash,
    #[serde(flatten)]
    status: TxStatus<BlobHash>,
}

impl<Z, K> StdBatchBuilder<Z, K>
where
    Z: RtAwareBatchBuilderSpec,
    K: KernelSlotHooks<Z::Spec, Z::Da> + Send + Sync + 'static,
{
    const DEFAULT_MAX_BATCH_SIZE_BYTES: usize = 1024 * 1024;

    /// Returns [`None`] if the transaction does not fit inside the batch.
    #[allow(clippy::type_complexity)]
    fn try_add_tx_to_batch(
        &self,
        seqdb_tx: &SeqDbTx,
        mut ctx: BatchConstructionContext<Z::Spec>,
    ) -> (
        BatchConstructionContext<Z::Spec>,
        Result<Option<TransactionReceipt<Z::Spec>>, TxProcessingError>,
    ) {
        // To fill a batch as big as possible, we only check if valid
        // tx can fit in the batch.
        let tx_len = seqdb_tx.tx_bytes.data.len();
        if ctx.current_batch_size_in_bytes + tx_len > self.max_batch_size_bytes() {
            return (ctx, Ok(None));
        }

        let tx_scratchpad = ctx.state_checkpoint.to_tx_scratchpad();
        let (res, tx_scratchpad) = process_tx(
            &self.runtime,
            &FullyBakedTx {
                data: seqdb_tx.tx_bytes.data.clone(),
            },
            &self.sequencer_address,
            &ctx.gas_price,
            ctx.visible_height,
            tx_scratchpad,
            ExecutionContext::Sequencer,
        );

        match res {
            Err(reason) => {
                // ...and immediately store the new `StateCheckpoint`.
                ctx.state_checkpoint = tx_scratchpad.revert();

                (ctx, Err(reason))
            }
            Ok(ApplyTxResult {
                receipt,
                transaction_consumption,
            }) => {
                let sequencer_reward = transaction_consumption.priority_fee();
                // ...and immediately store the new `StateCheckpoint`.
                ctx.state_checkpoint = tx_scratchpad.commit();
                ctx.reward.accumulate(sequencer_reward);

                (ctx, Ok(Some(receipt)))
            }
        }
    }

    fn mempool_cursor(&self, ctx: &BatchConstructionContext<Z::Spec>) -> MempoolCursor {
        MempoolCursor::new(
            self.max_batch_size_bytes()
                .saturating_sub(ctx.current_batch_size_in_bytes),
        )
    }

    fn max_batch_size_bytes(&self) -> usize {
        self.config
            .max_batch_size_bytes
            .unwrap_or(Self::DEFAULT_MAX_BATCH_SIZE_BYTES)
    }
}

#[async_trait]
impl<Z, K> BatchBuilder for StdBatchBuilder<Z, K>
where
    Z: RtAwareBatchBuilderSpec,
    K: Kernel<<Z::Spec as Spec>::Storage>
        + KernelWithSlotMapping<Z::Spec>
        + KernelSlotHooks<Z::Spec, Z::Da>,
{
    // The standard, non-preferred sequencer doesn't provide any information as
    // part of transaction confirmations. In the future, it might return
    // authentication gas usage information.
    type Confirmation = ();
    type Batch = Batch;
    type Config = StdBatchBuilderConfig;
    type Da = Z::Da;
    type Spec = Z::Spec;

    async fn create(
        storage_recv: StorageReceiver<Z::Spec>,
        sequencer_address: <Z::Da as DaSpec>::Address,
        seq_db_txs: Vec<SeqDbTx>,
        config: &StdBatchBuilderConfig,
    ) -> anyhow::Result<Self> {
        let kernel = Arc::new(K::default());
        let storage = storage_recv.borrow();

        let checkpoint = StateCheckpoint::new(storage.clone(), &*kernel);
        let (checkpoint_sender, checkpoint_receiver) = watch::channel(checkpoint);

        let api_state = ApiState::build(Arc::new(()), checkpoint_receiver, kernel.clone(), None);

        let checkpoint = StateCheckpoint::new(storage.clone(), &*kernel);
        let txsm = TxStatusManager::default();

        // We must drop it to retake ownership over `storage_recv`.
        drop(storage);

        Ok(Self {
            mempool: Mempool::new(
                txsm.clone(),
                config
                    .mempool_max_txs_count
                    .unwrap_or(default_mempool_max_txs_count()),
                seq_db_txs,
            )?,
            txsm,
            api_state,
            runtime: Z::Rt::default(),
            kernel,
            storage_recv,
            checkpoint_sender,
            checkpoint: Some(checkpoint),
            tx_hashes_of_last_batch: vec![],
            sequencer_address,
            config: config.clone(),
        })
    }

    fn is_ready(&self) -> bool {
        // Non-preferred sequencers are always ready to accept transactions.
        true
    }

    fn storage_receiver(&self) -> StorageReceiver<Self::Spec> {
        self.storage_recv.clone()
    }

    fn tx_status_manager(&self) -> TxStatusManager<Self::Da> {
        self.txsm.clone()
    }

    fn api_state(&self) -> ApiState<(), Self::Spec> {
        self.api_state.clone()
    }

    async fn set_state(&mut self, _da_height: u64, stf_state: <Z::Spec as Spec>::Storage) {
        let checkpoint = StateCheckpoint::new(stf_state, &*self.kernel);
        self.checkpoint_sender
            .send(checkpoint.clone_with_empty_witness())
            .ok();
        self.checkpoint = Some(checkpoint);
    }

    async fn accept_tx(
        &mut self,
        raw: RawTx,
    ) -> Result<AcceptedTx<Self::Confirmation>, AcceptTxError> {
        tracing::trace!(raw_tx = hex::encode(&raw), "`accept_tx` has been called");

        let authenticated = Z::Rt::add_standard_auth(raw);
        let baked_tx = {
            let data = borsh::to_vec(&authenticated).map_err(|err| AcceptTxError {
                http_status: StatusCode::BAD_REQUEST.as_u16(),
                title: "Failed to encode transaction".to_string(),
                details: err.to_string(),
            })?;
            FullyBakedTx { data }
        };

        if baked_tx.data.len() > self.max_batch_size_bytes() {
            return Err(AcceptTxError {
                http_status: StatusCode::PAYLOAD_TOO_LARGE.as_u16(),
                title: "Transaction is too big".to_string(),
                details: format!(
                    "Max allowed size: {}, submitted size: {}",
                    self.max_batch_size_bytes(),
                    baked_tx.data.len(),
                ),
            });
        }

        let state_checkpoint = self
            .checkpoint
            .take()
            .expect("Absent checkpoint; this is a bug, please report it");

        // This closure helps us make sure that we always put the
        // state checkpoint back into `self` at the end of the function.
        let (new_checkpoint, response) = (|mut checkpoint| {
            let gas_price = self.kernel.base_fee_per_gas(&mut checkpoint);
            let mut tx_scratchpad = checkpoint.to_tx_scratchpad();

            let (_, seq_stake_meter) = match self
                .runtime
                .sequencer_authorization()
                .authorize_sequencer(&self.sequencer_address, &gas_price, &mut tx_scratchpad)
            {
                Ok(ok) => ok,
                Err(AuthorizeSequencerError { reason }) => {
                    error!(%reason, "Sequencer authorization failed");

                    return (
                        tx_scratchpad.revert(),
                        Err(AcceptTxError {
                            http_status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                            title: "The sequencer is currently unavailable; contact the administrator if the problem persists".to_string(),
                            details: reason.to_string(),
                        }),
                    );
                }
            };

            let mut pre_exec_ws = tx_scratchpad.to_pre_exec_working_set(seq_stake_meter);

            let auth_res = match self.runtime.authenticate(&authenticated, &mut pre_exec_ws) {
                Ok(ok) => ok,
                Err(err) => {
                    let details = err.to_string();
                    let tx_scratchpad = self.runtime.sequencer_authorization().penalize_sequencer(
                        &self.sequencer_address,
                        err,
                        pre_exec_ws,
                    );
                    return (
                        tx_scratchpad.revert(),
                        Err(AcceptTxError {
                            // For certain kinds of authentication errors, 401
                            // or 403 would be more appropriate. But we'd have
                            // to inspect the error contents to determine the
                            // most appropriate status code... so 400 will do.
                            http_status: StatusCode::BAD_REQUEST.as_u16(),
                            title: "The transaction is invalid".to_string(),
                            details,
                        }),
                    );
                }
            };

            let tx_hash = auth_res.0.raw_tx_hash;
            let authenticated_tx = auth_res.0;

            let working_set = match pre_exec_ws
                .transfer_gas_to_working_set(&authenticated_tx.authenticated_tx)
            {
                Ok(ok) => ok,
                Err(AuthorizeTransactionError {
                    pre_exec_working_set,
                    reason,
                }) => {
                    let details = reason.to_string();
                    let tx_scratchpad = self.runtime.sequencer_authorization().penalize_sequencer(
                        &self.sequencer_address,
                        reason,
                        pre_exec_working_set,
                    );
                    return (
                        tx_scratchpad.revert(),
                        Err(AcceptTxError {
                            // Not enough gas, so 403 seems appropriate.
                            http_status: StatusCode::FORBIDDEN.as_u16(),
                            title: "Not enough gas for pre-execution checks".to_string(),
                            details,
                        }),
                    );
                }
            };

            {
                self.mempool.add_new_tx(tx_hash, baked_tx.clone());
                tracing::trace!(
                    %tx_hash,
                    "Transaction has been added to the mempool"
                );
            }

            (working_set.finalize().0.commit(), Ok(tx_hash))
        })(state_checkpoint);

        self.checkpoint = Some(new_checkpoint);
        let tx_hash = response?;

        Ok(AcceptedTx {
            tx: baked_tx,
            tx_hash,
            confirmation: (),
        })
    }

    async fn clear_batch(&mut self) -> anyhow::Result<()> {
        for tx_hash in self.tx_hashes_of_last_batch.drain(..) {
            self.mempool.remove_without_notifying(&tx_hash);
        }

        Ok(())
    }

    /// Builds a new batch of valid transactions in order they were added to mempool.
    /// Only transactions which are dispatched successfully are included in the batch.
    async fn build_next_batch(&mut self, _height: u64) -> anyhow::Result<FreshlyBuiltBatch<Self>> {
        tracing::debug!("build_next_batch has been called");

        let state_checkpoint = self.checkpoint.take().unwrap();
        let visible_height = state_checkpoint.rollup_height_to_access();

        // This closure helps us make sure that we always put the
        // `StateCheckpoint` back into `self` at the end of the function.
        let (new_checkpoint, response) = (|mut checkpoint| {
            let gas_price = self.kernel.base_fee_per_gas(&mut checkpoint);

            let mut ctx = BatchConstructionContext {
                visible_height,
                reward: SequencerReward::ZERO,
                gas_price,
                state_checkpoint: checkpoint,
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
                let (context, tx_receipt) = self.try_add_tx_to_batch(&mempool_tx, ctx);
                ctx = context;

                let tx_receipt = match tx_receipt {
                    Ok(txr) => txr,
                    Err(e) => match e {
                        // If authentication is fatally broken or the tx is for an unregistered sequencer, we can
                        // never submit it succesfully so drop it from the mempool
                        TxProcessingError::InvalidUnregisteredTx(_)
                        | TxProcessingError::InvalidRegisteredTx(
                            AuthenticationError::FatalError(_),
                        ) => {
                            tracing::info!(hash= %mempool_tx.hash, error = %e, "Invalid tx detected in mempool; dropping tx",);
                            self.mempool
                                .drop(&mempool_tx.hash, "Transaction is invalid".to_string());
                            continue;
                        }
                        // Otherwise, the issue has to do with the current state of the rollup, which could change at any point.
                        // We won't add the invalid tx to the batch, but we don't drop it either since it may become valid soon.
                        TxProcessingError::SequencerUnauthorized(_)
                        | TxProcessingError::InvalidRegisteredTx(AuthenticationError::OutOfGas(
                            _,
                        ))
                        | TxProcessingError::Skipped { .. } => {
                            tracing::info!(hash= %mempool_tx.hash, reason = %e, "The current state of the rollup didn't allow tx inclusion; ignoring tx",);
                            continue;
                        }
                    },
                };

                match tx_receipt.map(|r| r.receipt) {
                    Some(TxEffect::Successful(_)) => {
                        tracing::info!(
                            hash = %mempool_tx.hash,
                            "Transaction has been included in the batch",
                        );

                        let tx_len = mempool_tx.tx_bytes.data.len();
                        ctx.current_batch_size_in_bytes += tx_len;

                        txs.push(TxWithHash {
                            fully_baked_tx: mempool_tx.tx_bytes.clone(),
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
                            tx = hex::encode(&mempool_tx.tx_bytes.data),
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

            // TODO SEQUENCER; don't drain txs from mempool until the batch is submitted
            for tx in &txs {
                self.mempool.remove_without_notifying(&tx.hash);
            }

            if txs.is_empty() {
                return (
                    ctx.state_checkpoint,
                    Err(anyhow::anyhow!(
                        "No valid transactions are available out of {} were in the pool",
                        count_before
                    )),
                );
            }

            tracing::info!(
                txs_count = txs.len(),
                "Batch of transactions has been built"
            );

            let (txs, hashes) = txs
                .into_iter()
                .map(|tx| (tx.fully_baked_tx, tx.hash))
                .unzip();

            (
                ctx.state_checkpoint,
                Ok(FreshlyBuiltBatch {
                    inner: Batch { txs },
                    hashes,
                }),
            )
        })(state_checkpoint);

        self.checkpoint = Some(new_checkpoint);
        response
    }
}

struct BatchConstructionContext<S: Spec> {
    state_checkpoint: StateCheckpoint<S::Storage>,
    visible_height: u64,
    reward: SequencerReward,
    gas_price: <S::Gas as Gas>::Price,
    current_batch_size_in_bytes: usize,
}

const fn default_mempool_max_txs_count() -> NonZero<usize> {
    match NonZero::new(100) {
        Some(default) => default,
        None => panic!("100 is greater than 0"),
    }
}
