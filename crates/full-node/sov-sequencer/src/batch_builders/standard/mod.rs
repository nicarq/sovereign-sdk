//! Standard, "vanilla" non-preferred sequencer implementation.

mod mempool;

use std::marker::PhantomData;
use std::num::NonZero;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use schemars::JsonSchema;
use serde::Deserialize;
use sov_modules_api::capabilities::{
    AuthenticationError, ChainState, HasKernel, TransactionAuthenticator,
};
use sov_modules_api::rest::{ApiState, StateUpdateReceiver};
use sov_modules_api::transaction::SequencerReward;
use sov_modules_api::{
    Batch, ExecutionContext, FullyBakedTx, Gas, GasMeter, NestedEnumUtils, RawTx, Spec,
    StateCheckpoint, StateProvider, VersionReader, WorkingSet,
};
use sov_modules_stf_blueprint::{
    process_tx, ApplyTxResult, PreExecError, TransactionReceipt, TxEffect, TxProcessingError,
    ValidatedAuthOutput,
};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::node::DaSyncState;
use thiserror::Error;
use tokio::sync::watch;
use tracing::error;

use self::mempool::{Mempool, MempoolCursor};
use super::{sender_is_allowed, EmptyConfirmation, RtAwareBatchBuilderSpec};
use crate::batch_builders::{
    pre_exec_err_to_accept_tx_err, tx_auth, AcceptTxError, AcceptedTx, BatchBuilder,
    FreshlyBuiltBatch, StateUpdateInfo, TxWithHash,
};
use crate::sequencer::SequencerNotReadyDetails;
use crate::{SeqDbTx, SeqDbTxExtend, TxHash, TxStatus, TxStatusManager};

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
pub struct StdBatchBuilder<Z: RtAwareBatchBuilderSpec> {
    runtime: Z::Rt,
    txsm: TxStatusManager<<Z::Spec as Spec>::Da>,
    mempool: Mempool<Self>,
    checkpoint: Option<StateCheckpoint<<Z::Spec as Spec>::Storage>>,
    checkpoint_sender: watch::Sender<StateCheckpoint<<Z::Spec as Spec>::Storage>>,
    api_state: ApiState<Z::Spec>,
    state_update_recv: StateUpdateReceiver<<Z::Spec as Spec>::Storage>,
    tx_hashes_of_last_batch: Vec<TxHash>,
    sequencer_address: <<Z::Spec as Spec>::Da as DaSpec>::Address,
    admin_addresses: Vec<<Z::Spec as Spec>::Address>,
    config: StdBatchBuilderConfig,
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
    const DEFAULT_MAX_BATCH_SIZE_BYTES: usize = 1024 * 1024;

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
        let fully_baked = seqdb_tx.fully_baked_tx();

        // To fill a batch as big as possible, we only check if valid
        // tx can fit in the batch.
        let tx_len = fully_baked.data.len();
        if ctx.current_batch_size_in_bytes + tx_len > self.max_batch_size_bytes() {
            return (ctx, Ok(None));
        }

        let tx_scratchpad = ctx.state_checkpoint.to_tx_scratchpad();

        let (tx_scratchpad, output_res) = tx_auth(
            &self.runtime,
            tx_scratchpad,
            ctx.gas_price.clone(),
            &self.sequencer_address,
            seqdb_tx.tx_input::<Self>(),
        );

        let (auth_output, gas_meter) = match output_res {
            Ok(ok) => ok,

            Err(err) => {
                ctx.state_checkpoint = tx_scratchpad.revert();
                return (ctx, Err(AddTxToBatchError::PreExecCheck(err)));
            }
        };

        let gas_info = gas_meter.gas_info();
        let (_, authz_data, message) = &auth_output;

        // If the module isn't sequencer safe, the caller must be an admin to proceed
        if !sender_is_allowed(
            &self.runtime,
            message,
            authz_data.default_address.as_ref(),
            &self.sequencer_address,
            &self.admin_addresses,
        ) {
            ctx.state_checkpoint = tx_scratchpad.revert();
            return (
                ctx,
                Err(AddTxToBatchError::PermissionDenied(
                    message.discriminant().into(),
                )),
            );
        }

        let (res, tx_scratchpad) = process_tx(
            &self.runtime,
            ValidatedAuthOutput::Valid(auth_output),
            &gas_info.gas_price,
            &gas_info.gas_used,
            &self.sequencer_address,
            ctx.visible_height,
            tx_scratchpad,
            ExecutionContext::Sequencer,
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

                (ctx, Ok(Some((fully_baked, receipt))))
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
impl<Z> BatchBuilder for StdBatchBuilder<Z>
where
    Z: RtAwareBatchBuilderSpec,
{
    type TxInput = <Z::Rt as TransactionAuthenticator<Z::Spec>>::Input;
    // The standard, non-preferred sequencer doesn't provide any information as
    // part of transaction confirmations. In the future, it might return
    // authentication gas usage information.
    type Confirmation = EmptyConfirmation<Z>;
    type Batch = Batch;
    type Config = StdBatchBuilderConfig;
    type Spec = Z::Spec;

    async fn create(
        state_update_recv: StateUpdateReceiver<<Z::Spec as Spec>::Storage>,
        _da_sync_state: Arc<DaSyncState>,
        sequencer_address: <<Z::Spec as Spec>::Da as DaSpec>::Address,
        seq_db_txs: Vec<SeqDbTx>,
        admin_addresses: Vec<<Z::Spec as Spec>::Address>,
        config: &StdBatchBuilderConfig,
        _last_event_number: u64,
    ) -> anyhow::Result<Self> {
        let runtime = Z::Rt::default();
        let kernel_with_slot_mapping = runtime.kernel_with_slot_mapping();
        let kernel = runtime.kernel();

        let state_update_ref = state_update_recv.borrow();

        let checkpoint = StateCheckpoint::new(state_update_ref.storage.clone(), &kernel);
        let (checkpoint_sender, checkpoint_receiver) = watch::channel(checkpoint);

        let api_state = ApiState::build(
            Arc::new(()),
            checkpoint_receiver,
            kernel_with_slot_mapping,
            None,
        );

        let checkpoint = StateCheckpoint::new(state_update_ref.storage.clone(), &kernel);
        let txsm = TxStatusManager::default();

        // We must drop it to retake ownership over `storage_recv`.
        drop(state_update_ref);

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
            state_update_recv,
            admin_addresses,
            checkpoint_sender,
            checkpoint: Some(checkpoint),
            tx_hashes_of_last_batch: vec![],
            sequencer_address,
            config: config.clone(),
        })
    }

    fn encode_tx(raw: RawTx) -> Self::TxInput {
        Z::Rt::add_standard_auth(raw)
    }

    fn is_ready(&self) -> Result<(), SequencerNotReadyDetails> {
        Ok(())
    }

    fn state_update_receiver(&self) -> StateUpdateReceiver<<Self::Spec as Spec>::Storage> {
        self.state_update_recv.clone()
    }

    fn tx_status_manager(&self) -> TxStatusManager<<Z::Spec as Spec>::Da> {
        self.txsm.clone()
    }

    fn api_state(&self) -> ApiState<Self::Spec> {
        self.api_state.clone()
    }

    async fn update_state(
        &mut self,
        StateUpdateInfo {
            storage,
            rollup_height,
            ..
        }: StateUpdateInfo<<Z::Spec as Spec>::Storage>,
    ) {
        let checkpoint = StateCheckpoint::new(storage, &Z::Rt::default().kernel());

        tracing::debug!(
            da_height = rollup_height,
            "The sequencer received a new state. Notifying the subscribers."
        );

        self.checkpoint_sender
            .send(checkpoint.clone_with_empty_witness())
            .ok();
        self.checkpoint = Some(checkpoint);
    }

    async fn accept_tx(
        &mut self,
        tx_input: Self::TxInput,
    ) -> Result<AcceptedTx<Self::Confirmation>, AcceptTxError> {
        let baked_tx = {
            FullyBakedTx {
                data: borsh::to_vec(&tx_input).map_err(|err| AcceptTxError {
                    http_status: StatusCode::BAD_REQUEST.as_u16(),
                    title: "Failed to encode transaction".to_string(),
                    details: err.to_string(),
                })?,
            }
        };

        tracing::trace!(
            baked_tx = hex::encode(&baked_tx),
            "`accept_tx` has been called"
        );

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
        let (new_checkpoint, response) = (|mut checkpoint: StateCheckpoint<
            <Z::Spec as Spec>::Storage,
        >| {
            let gas_price = self.runtime.chain_state().base_fee_per_gas(&mut checkpoint).expect("Impossible to get the gas price for the current slot. This is a bug. Please report it");

            let (tx_scratchpad, output_res) = tx_auth(
                &self.runtime,
                checkpoint.to_tx_scratchpad(),
                gas_price,
                &self.sequencer_address,
                tx_input.clone(),
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
            let mut working_set = WorkingSet::create_working_set(
                tx_scratchpad,
                &gas_info.gas_price,
                &auth_output.0.authenticated_tx,
            );

            if let Err(err) = working_set.charge_gas(&gas_info.gas_used) {
                let (scratchpad, _) = working_set.revert();

                return (
                    scratchpad.revert(),
                    Err(AcceptTxError {
                        // Not enough gas, so 403 seems appropriate.
                        http_status: StatusCode::FORBIDDEN.as_u16(),
                        title: "Not enough gas for pre-execution checks".to_string(),
                        details: err.to_string(),
                    }),
                );
            };

            {
                self.mempool.add_new_tx(tx_hash, tx_input);
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
            confirmation: EmptyConfirmation(PhantomData),
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
    async fn build_next_batch(
        &mut self,
        _height: u64,
        _sequence_number: u64,
    ) -> anyhow::Result<FreshlyBuiltBatch<Self>> {
        tracing::debug!("build_next_batch has been called");

        let state_checkpoint = self.checkpoint.take().unwrap();
        let visible_height = state_checkpoint.rollup_height_to_access();

        // This closure helps us make sure that we always put the
        // `StateCheckpoint` back into `self` at the end of the function.
        let (new_checkpoint, response) = (|mut checkpoint| {
            let gas_price = self.runtime.chain_state().base_fee_per_gas(&mut checkpoint).expect("Impossible to get the gas price for the current slot. This is a bug. Please report it");

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
                    Err(add_tx_error) => match add_tx_error {
                        AddTxToBatchError::PreExecCheck(pre_exec_error) => match pre_exec_error {
                            // If authentication is fatally broken, we can
                            // never submit it succesfully so drop it from the mempool
                            PreExecError::AuthError(AuthenticationError::FatalError(_, _)) => {
                                tracing::info!(hash= %mempool_tx.hash, error = %pre_exec_error, "Invalid tx detected in mempool; dropping tx",);
                                self.mempool
                                    .drop(&mempool_tx.hash, "Transaction is invalid".to_string());
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
                            self.mempool
                                .drop(&mempool_tx.hash, add_tx_error.to_string());
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
