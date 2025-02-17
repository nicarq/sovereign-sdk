//! Standard, "vanilla" non-preferred sequencer implementation.

mod db;
mod mempool;

use std::marker::PhantomData;
use std::num::NonZero;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use db::StandardBbDb;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sov_modules_api::capabilities::{
    AuthenticationError, ChainState, HasKernel, TransactionAuthenticator,
};
use sov_modules_api::rest::utils::ErrorObject;
use sov_modules_api::rest::ApiState;
use sov_modules_api::transaction::SequencerReward;
use sov_modules_api::{
    ExecutionContext, FullyBakedTx, Gas, GasArray, GasMeter, NestedEnumUtils, NoOpControlFlow,
    RawTx, Spec, StateCheckpoint, StateProvider, WorkingSet,
};
use sov_modules_stf_blueprint::{
    process_tx_and_reward_sequencer, ApplyTxResult, PreExecError, TransactionReceipt, TxEffect,
    TxProcessingError,
};
use sov_rest_utils::json_obj;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::node::DaSyncState;
use thiserror::Error;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tracing::error;

use self::mempool::{Mempool, MempoolCursor};
use super::{sender_is_allowed, EmptyConfirmation, RtAwareBatchBuilderSpec, SeqDbTx};
use crate::batch_builders::{
    pre_exec_err_to_accept_tx_err, tx_auth, AcceptedTx, BatchBuilder, StateUpdateInfo,
    WithCachedTxHashes,
};
use crate::sequencer::SequencerNotReadyDetails;
use crate::{SequencerConfig, TxHash, TxStatus, TxStatusManager};

struct Inner<Z: RtAwareBatchBuilderSpec> {
    assembled_batch: Option<WithCachedTxHashes<Vec<FullyBakedTx>>>,
    checkpoint: Option<StateCheckpoint<Z::Spec>>,
    mempool: Mempool<StdBatchBuilder<Z>>,
}

/// Configuration for [`StdBatchBuilder`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct StdBatchBuilderConfig {
    /// Maximum number of transactions in mempool. Once this limit is reached,
    /// the batch builder will evict older transactions.
    pub mempool_max_txs_count: Option<NonZero<usize>>,
    /// Maximum size of a batch. The batch builder will not build batches larger
    /// than this size.
    pub max_batch_size_bytes: Option<NonZero<usize>>,
}

/// A [`BatchBuilder`] that creates batches of transactions in a way that's
/// reasonably "fair" to everybody.
///
/// Transactions are included in batches by following a largest-first,
/// least-recent-first priority. Only transactions that were successfully
/// dispatched are included.
pub struct StdBatchBuilder<Z: RtAwareBatchBuilderSpec> {
    runtime: Z::Rt,
    txsm: TxStatusManager<<Z::Spec as Spec>::Da>,
    inner: Mutex<Inner<Z>>,
    checkpoint_sender: watch::Sender<StateCheckpoint<Z::Spec>>,
    api_state: ApiState<Z::Spec>,
    config:
        SequencerConfig<<Z::Spec as Spec>::Da, <Z::Spec as Spec>::Address, StdBatchBuilderConfig>,
}

/// An error that indicates that the transaction could not be added to the batch.
#[derive(Debug, Error)]
enum AddTxToBatchError {
    /// Error occurred during transaction processing.
    #[error("Error occurred during transaction processing")]
    TxProcessing(#[source] TxProcessingError),
    /// Error occurred during pre-execution checks.
    #[error("Error occurred during pre-execution checks")]
    PreExecCheck(#[source] PreExecError),
    /// The transaction was rejected because it is not sequencer safe.
    #[error("The transaction was rejected because the it invokes the `{0}` module which may modify sequencer configurations. You need admin permissions on the sequencer to call this method.")]
    PermissionDenied(&'static str),
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct TxInfo<BlobHash> {
    id: TxHash,
    #[serde(flatten)]
    status: TxStatus<BlobHash>,
}

impl<Z> StdBatchBuilder<Z>
where
    Z: RtAwareBatchBuilderSpec,
{
    /// Returns [`None`] if the transaction does not fit inside the batch.
    #[allow(clippy::type_complexity)]
    fn try_add_tx_to_batch(
        &self,
        seqdb_tx: &SeqDbTx,
        mut ctx: BatchConstructionContext<Z::Spec>,
    ) -> (
        BatchConstructionContext<Z::Spec>,
        Result<Option<(FullyBakedTx, TransactionReceipt<Z::Spec>)>, AddTxToBatchError>,
    ) {
        let tx = seqdb_tx.tx.clone();

        // To fill a batch as big as possible, we only check if valid
        // tx can fit in the batch.
        let tx_len = tx.data.len();
        if ctx.current_batch_size_in_bytes + tx_len > self.max_batch_size_bytes().get() {
            return (ctx, Ok(None));
        }

        let tx_scratchpad = ctx.state_checkpoint.to_tx_scratchpad();

        let (tx_scratchpad, output_res) = tx_auth(
            &self.runtime,
            tx_scratchpad,
            ctx.gas_price.clone(),
            &self.config.da_address,
            &seqdb_tx.tx,
        );

        let (auth_output, gas_meter) = match output_res {
            Ok(ok) => ok,

            Err(err) => {
                ctx.state_checkpoint = tx_scratchpad.revert();
                return (ctx, Err(AddTxToBatchError::PreExecCheck(err)));
            }
        };

        let (_, authz_data, message) = &auth_output;

        // If the module isn't sequencer safe, the caller must be an admin to proceed
        if !sender_is_allowed(
            &self.runtime,
            message,
            authz_data.default_address.as_ref(),
            &self.config.da_address,
            &self.config.admin_addresses,
        ) {
            ctx.state_checkpoint = tx_scratchpad.revert();
            return (
                ctx,
                Err(AddTxToBatchError::PermissionDenied(
                    message.discriminant().into(),
                )),
            );
        }

        let pre_exec_working_set = tx_scratchpad.to_pre_exec_working_set(gas_meter);
        let (res, tx_scratchpad, _gas_meter) = process_tx_and_reward_sequencer(
            &self.runtime,
            pre_exec_working_set,
            // Currently the sequencer doesn't take into account the slot gas limit.
            &<<Z::Spec as Spec>::Gas>::MAX,
            auth_output,
            &self.config.da_address,
            self.config.rollup_address.clone(),
            ExecutionContext::Sequencer,
            &NoOpControlFlow,
        );

        match res {
            Err(reason) => {
                // ...and immediately store the new `StateCheckpoint`.
                ctx.state_checkpoint = tx_scratchpad.revert();
                (ctx, Err(AddTxToBatchError::TxProcessing(reason)))
            }
            Ok(ApplyTxResult {
                receipt,
                transaction_consumption,
            }) => {
                let sequencer_reward = transaction_consumption.priority_fee();
                // ...and immediately store the new `StateCheckpoint`.
                ctx.state_checkpoint = tx_scratchpad.commit();
                ctx.reward.accumulate(sequencer_reward);

                (ctx, Ok(Some((tx, receipt))))
            }
        }
    }

    fn mempool_cursor(&self, ctx: &BatchConstructionContext<Z::Spec>) -> MempoolCursor {
        MempoolCursor::new(
            self.max_batch_size_bytes()
                .get()
                .saturating_sub(ctx.current_batch_size_in_bytes),
        )
    }

    fn max_batch_size_bytes(&self) -> NonZero<usize> {
        self.config
            .batch_builder
            .max_batch_size_bytes
            .unwrap_or(self.default_max_batch_size_bytes())
    }

    fn default_max_batch_size_bytes(&self) -> NonZero<usize> {
        // 1 MiB
        NonZero::new(1024 * 1024).unwrap()
    }
}

#[async_trait]
impl<Z> BatchBuilder for StdBatchBuilder<Z>
where
    Z: RtAwareBatchBuilderSpec,
{
    // The standard, non-preferred sequencer doesn't provide any information as
    // part of transaction confirmations. In the future, it might return
    // authentication gas usage information.
    type Confirmation = EmptyConfirmation<Z::Rt>;
    type Batch = Vec<FullyBakedTx>;
    type Config = StdBatchBuilderConfig;
    type Spec = Z::Spec;

    // Batches coming from non-preferred sequencers lack sequence numbers, so
    // are susceptible to issues when submitted and processed out-of-order...
    // unless the DA adapter guarantees ordering.
    const PARALLEL_DA_SUBMISSION: bool =
        <Z::DaService as DaService>::GUARANTEES_TRANSACTION_ORDERING;

    async fn create(
        latest_state_update: StateUpdateInfo<<Z::Spec as Spec>::Storage>,
        txsm: TxStatusManager<<Self::Spec as Spec>::Da>,
        _da_sync_state: Arc<DaSyncState>,
        storage_path: &Path,
        config: &SequencerConfig<<Z::Spec as Spec>::Da, <Z::Spec as Spec>::Address, Self::Config>,
    ) -> anyhow::Result<(Self, Option<JoinHandle<()>>)> {
        let runtime = Z::Rt::default();
        let kernel_with_slot_mapping = runtime.kernel_with_slot_mapping();

        let checkpoint =
            StateCheckpoint::new(latest_state_update.storage.clone(), &runtime.kernel());
        let (checkpoint_sender, checkpoint_receiver) = watch::channel(checkpoint);

        let api_state = ApiState::build(
            Arc::new(()),
            checkpoint_receiver,
            kernel_with_slot_mapping,
            None,
        );

        let checkpoint =
            StateCheckpoint::new(latest_state_update.storage.clone(), &runtime.kernel());
        let inner = Inner {
            checkpoint: Some(checkpoint),
            assembled_batch: None,
            mempool: Mempool::new(
                txsm.clone(),
                config
                    .batch_builder
                    .mempool_max_txs_count
                    .unwrap_or(default_mempool_max_txs_count()),
                StandardBbDb::new(storage_path).await?,
            )?,
        };

        Ok((
            Self {
                inner: inner.into(),
                txsm,
                api_state,
                runtime: Z::Rt::default(),
                checkpoint_sender,
                config: config.clone(),
            },
            None,
        ))
    }

    fn encode_tx(raw: RawTx) -> FullyBakedTx {
        Z::Rt::encode_with_standard_auth(raw)
    }

    fn is_ready(&self) -> Result<(), SequencerNotReadyDetails> {
        // The non-preferred batch builder is always ready to accept
        // transactions.
        Ok(())
    }

    fn api_state(&self) -> ApiState<Self::Spec> {
        self.api_state.clone()
    }

    async fn update_state(
        &self,
        StateUpdateInfo {
            storage,
            slot_number,
            ..
        }: StateUpdateInfo<<Z::Spec as Spec>::Storage>,
    ) {
        let checkpoint = StateCheckpoint::new(storage, &Z::Rt::default().kernel());

        tracing::debug!(
            %slot_number,
            "The sequencer received a new state. Notifying the subscribers."
        );

        self.checkpoint_sender
            .send(checkpoint.clone_with_empty_witness())
            .ok();
        let mut inner = self.inner.lock().await;
        inner.checkpoint = Some(checkpoint);
    }

    async fn accept_tx(
        &self,
        baked_tx: FullyBakedTx,
    ) -> Result<AcceptedTx<Self::Confirmation>, ErrorObject> {
        tracing::trace!(
            baked_tx = hex::encode(&baked_tx),
            "`accept_tx` has been called"
        );

        if baked_tx.data.len() > self.max_batch_size_bytes().get() {
            return Err(ErrorObject {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                title: "Transaction is too big".to_string(),
                details: json_obj!({
                    "max_allowed_size": self.max_batch_size_bytes(),
                    "submitted_size": baked_tx.data.len(),
                }),
            });
        }

        let mut inner = self.inner.lock().await;
        let state_checkpoint = inner
            .checkpoint
            .take()
            .expect("Absent checkpoint; this is a bug, please report it");

        // This closure helps us make sure that we always put the
        // state checkpoint back into `self` at the end of the function.
        let (new_checkpoint, response) = (|mut checkpoint: StateCheckpoint<Z::Spec>| {
            let gas_price = self.runtime.chain_state().base_fee_per_gas(&mut checkpoint).expect("Impossible to get the gas price for the current slot. This is a bug. Please report it");

            let (tx_scratchpad, output_res) = tx_auth(
                &self.runtime,
                checkpoint.to_tx_scratchpad(),
                gas_price,
                &self.config.da_address,
                &baked_tx.clone(),
            );

            let (auth_output, gas_meter) = match output_res {
                Ok(ok) => ok,
                Err(error) => {
                    return (
                        tx_scratchpad.revert(),
                        Err(pre_exec_err_to_accept_tx_err(error)),
                    );
                }
            };

            let tx_hash = auth_output.0.raw_tx_hash;

            let gas_info = gas_meter.gas_info();
            let tx = auth_output.0.authenticated_tx;

            let working_set_gas_meter =
                tx.gas_meter(&gas_info.gas_price.clone(), &<<Z::Spec as Spec>::Gas>::MAX);
            let mut working_set =
                WorkingSet::create_working_set(tx_scratchpad, &tx, working_set_gas_meter);

            if let Err(err) = working_set.charge_gas(&gas_info.gas_used) {
                let (scratchpad, _) = working_set.revert();

                return (
                    scratchpad.revert(),
                    Err(ErrorObject {
                        // Not enough gas, so 403 seems appropriate.
                        status: StatusCode::FORBIDDEN,
                        title: "Not enough gas for pre-execution checks".to_string(),
                        details: json_obj!({
                            "message": err.to_string()
                        }),
                    }),
                );
            };

            (working_set.finalize().0.commit(), Ok(tx_hash))
        })(state_checkpoint);

        let tx_hash = response?;
        {
            inner.mempool.add_new_tx(tx_hash, baked_tx.clone());
            tracing::trace!(
                %tx_hash,
                "Transaction has been added to the mempool"
            );
        }

        inner.checkpoint = Some(new_checkpoint);

        Ok(AcceptedTx {
            tx: baked_tx,
            tx_hash,
            confirmation: EmptyConfirmation(PhantomData),
        })
    }

    async fn tx_status(
        &self,
        tx_hash: &TxHash,
    ) -> anyhow::Result<
        TxStatus<<<Self::Spec as Spec>::Da as sov_modules_api::DaSpec>::TransactionId>,
    > {
        if let Some(status) = self.txsm.get_cached(tx_hash) {
            return Ok(status);
        }

        let inner = self.inner.lock().await;

        if inner.mempool.contains(tx_hash) {
            Ok(TxStatus::Submitted)
        } else {
            Ok(TxStatus::Unknown)
        }
    }

    /// Builds a new batch of valid transactions in order they were added to mempool.
    /// Only transactions which are dispatched successfully are included in the batch.
    async fn assemble_batch(&self) -> anyhow::Result<Option<()>> {
        tracing::debug!("`assemble_batch` has been called");
        let mut inner = self.inner.lock().await;

        // We already have a batch assembled. We'll wait until it's popped
        // before assembling a new one.
        if inner.assembled_batch.is_some() {
            return Ok(None);
        }

        let state_checkpoint = inner.checkpoint.take().unwrap();
        let mempool = &mut inner.mempool;

        // This closure helps us make sure that we always put the
        // `StateCheckpoint` back into `self` at the end of the function.
        let (new_checkpoint, response) = (|mut checkpoint| {
            let gas_price = self.runtime.chain_state().base_fee_per_gas(&mut checkpoint).expect("Impossible to get the gas price for the current slot. This is a bug. Please report it");

            let mut ctx = BatchConstructionContext {
                reward: SequencerReward::ZERO,
                gas_price,
                state_checkpoint: checkpoint,
                current_batch_size_in_bytes: 0,
            };

            let mut txs = Vec::new();

            let count_before = mempool.len();
            tracing::debug!(
                txs_count = count_before,
                "Going to build batch from transactions in mempool"
            );

            let mut cursor = self.mempool_cursor(&ctx);

            while let Some(mempool_tx) = mempool.next(&mut cursor) {
                let (context, tx_receipt) = self.try_add_tx_to_batch(&mempool_tx, ctx);
                ctx = context;

                let tx_receipt = match tx_receipt {
                    Ok(txr) => txr,
                    Err(add_tx_error) => match add_tx_error {
                        AddTxToBatchError::PreExecCheck(pre_exec_error) => match pre_exec_error {
                            // If authentication is fatally broken, we can
                            // never submit it succesfully so drop it from the mempool
                            PreExecError::AuthError(AuthenticationError::FatalError(_, _)) => {
                                tracing::info!(hash= %mempool_tx.hash, error = %pre_exec_error, "Invalid tx detected in mempool; dropping tx",);
                                mempool.drop_and_notify(
                                    &mempool_tx.hash,
                                    "Transaction is invalid".to_string(),
                                );
                                continue;
                            }
                            PreExecError::SequencerError(_)
                            | PreExecError::AuthError(AuthenticationError::OutOfGas(_)) => {
                                tracing::info!(hash= %mempool_tx.hash, reason = %pre_exec_error, "The current state of the rollup didn't allow tx inclusion; ignoring tx",);
                                continue;
                            }
                        },
                        AddTxToBatchError::TxProcessing(tx_processing_error) => {
                            tracing::info!(hash= %mempool_tx.hash, reason = %tx_processing_error, "The current state of the rollup didn't allow tx inclusion; ignoring tx",);
                            continue;
                        }
                        AddTxToBatchError::PermissionDenied(module) => {
                            mempool.drop_and_notify(&mempool_tx.hash, add_tx_error.to_string());
                            tracing::info!(hash= %mempool_tx.hash, target_module = %module, "Tx attempted to invoke a sequencer-unsafe module without appropriate permissions; dropping tx");
                            continue;
                        }
                    },
                };

                match tx_receipt.map(|(tx, r)| (tx, r.receipt)) {
                    Some((fully_baked_tx, TxEffect::Successful(_))) => {
                        tracing::info!(
                            hash = %mempool_tx.hash,
                            "Transaction has been included in the batch",
                        );

                        ctx.current_batch_size_in_bytes += fully_baked_tx.data.len();

                        txs.push(TxWithHash {
                            fully_baked_tx,
                            hash: mempool_tx.hash,
                        });

                        // Update the cursor to reflect the new amount of available
                        // space inside the batch.
                        cursor = cursor.max(self.mempool_cursor(&ctx));
                    }
                    Some((fully_baked_tx, tx_receipt)) => {
                        // Failed transaction; ignore and process the next one.
                        tracing::warn!(
                            ?tx_receipt,
                            tx = hex::encode(&fully_baked_tx),
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

            if txs.is_empty() {
                return (ctx.state_checkpoint, Ok(None));
            }

            tracing::info!(
                txs_count = txs.len(),
                "Batch of transactions has been built"
            );

            let (txs, tx_hashes) = txs
                .into_iter()
                .map(|tx| (tx.fully_baked_tx, tx.hash))
                .unzip();

            (
                ctx.state_checkpoint,
                Ok(Some(WithCachedTxHashes {
                    inner: txs,
                    tx_hashes,
                })),
            )
        })(state_checkpoint);

        inner.checkpoint = Some(new_checkpoint);

        match response {
            Ok(batch) => {
                inner.assembled_batch = batch;
                // Return Some(()) if and only if the batch is some
                Ok(inner.assembled_batch.as_ref().map(|_| ()))
            }
            Err(e) => Err(e),
        }
    }

    async fn peek_batches(&self) -> anyhow::Result<Vec<WithCachedTxHashes<Self::Batch>>> {
        let inner = self.inner.lock().await;
        if let Some(batch) = &inner.assembled_batch {
            Ok(vec![batch.clone()])
        } else {
            Ok(vec![])
        }
    }

    async fn pop_batch(&self) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().await;
        if let Some(batch) = inner.assembled_batch.take() {
            for tx_hash in batch.tx_hashes {
                // We're not dropping transactions because we're evicting them,
                // but rather because we don't need them anymore after
                // submitting them. Thus, sending a "drop" notification to users
                // would be semantically wrong.
                inner.mempool.drop_without_notifying(&tx_hash);
            }
        }

        Ok(())
    }
}

struct BatchConstructionContext<S: Spec> {
    state_checkpoint: StateCheckpoint<S>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct TxWithHash {
    fully_baked_tx: FullyBakedTx,
    hash: TxHash,
}
