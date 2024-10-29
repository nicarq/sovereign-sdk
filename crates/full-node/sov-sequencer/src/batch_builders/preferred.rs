//! See [`PreferredBatchBuilder`].

use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use serde_with::serde_as;
use sov_blob_storage::PreferredBatchData;
use sov_modules_api::capabilities::{ChainState, HasKernel, TransactionAuthenticator};
use sov_modules_api::rest::{ApiState, StorageReceiver};
use sov_modules_api::{
    Batch, DaSpec, ExecutionContext, FullyBakedTx, GasMeter, KernelStateAccessor, NestedEnumUtils,
    RawTx, RuntimeEventProcessor, RuntimeEventResponse, Spec, StateCheckpoint,
};
use sov_modules_stf_blueprint::{
    process_tx, ApplyTxResult, TransactionReceipt, TxEffect, ValidatedAuthOutput,
};
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::TxHash;
use tokio::sync::watch;
use tracing::{error, info, trace};

use super::{
    pre_exec_err_to_accept_tx_err, sender_is_allowed, tx_auth, DataWithEvents,
    RtAwareBatchBuilderSpec,
};
use crate::batch_builders::{
    AcceptTxError, AcceptedTx, BatchBuilder, FreshlyBuiltBatch, TxWithHash,
};
use crate::{SeqDbTx, TxStatusManager};

#[serde_with::serde_as]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TxBody(#[serde_as(as = "serde_with::base64::Base64")] Vec<u8>);

/// Transaction confirmation data of [`PreferredBatchBuilder`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Confirmation<Z: RtAwareBatchBuilderSpec> {
    tx_hash: TxHash,
    tx: Option<TxBody>,
    events: Vec<RuntimeEventResponse<<Z::Rt as RuntimeEventProcessor>::RuntimeEvent>>,
    receipt: TxEffect<Z::Spec>,
}

impl<Z: RtAwareBatchBuilderSpec> DataWithEvents for Confirmation<Z> {
    type EventInner = <Z::Rt as RuntimeEventProcessor>::RuntimeEvent;

    fn events(&self) -> Vec<RuntimeEventResponse<Self::EventInner>> {
        self.events.clone()
    }
}

impl<Z: RtAwareBatchBuilderSpec> TryFrom<TransactionReceipt<Z::Spec>> for Confirmation<Z> {
    type Error = anyhow::Error;

    fn try_from(value: TransactionReceipt<Z::Spec>) -> Result<Self, Self::Error> {
        Ok(Self {
            tx_hash: value.tx_hash,
            tx: value.body_to_save.map(TxBody),
            events: value
                .events
                .into_iter()
                .enumerate()
                .map(|(idx, event)| <RuntimeEventResponse<<Z::Rt as RuntimeEventProcessor>::RuntimeEvent>>::try_from((idx as u64, event)))
                .collect::<anyhow::Result<Vec<_>>>()?,
            receipt: value.receipt,
        })
    }
}

/// A batch builder with instant transaction confirmation.
pub struct PreferredBatchBuilder<Z: RtAwareBatchBuilderSpec> {
    runtime: Z::Rt,
    storage: StorageReceiver<Z::Spec>,
    sequencer_address: <<Z::Spec as Spec>::Da as DaSpec>::Address,
    checkpoint: Option<StateCheckpoint<<Z::Spec as Spec>::Storage>>,
    checkpoint_sender: watch::Sender<StateCheckpoint<<Z::Spec as Spec>::Storage>>,
    api_state: ApiState<Z::Spec>,
    da_sync_state: Arc<DaSyncState>,
    last_da_height: u64,
    txs_in_next_batch: Vec<TxWithHash>,
    admin_addresses: Vec<<Z::Spec as Spec>::Address>,
}

impl<Z: RtAwareBatchBuilderSpec> PreferredBatchBuilder<Z> {
    async fn accept_tx_internal(
        &mut self,
        tx_input: <Self as BatchBuilder>::TxInput,
    ) -> Result<AcceptedTx<Confirmation<Z>>, AcceptTxError> {
        let state_checkpoint = self
            .checkpoint
            .take()
            .expect("Absent checkpoint; this is a bug, please report it");

        let baked_tx = FullyBakedTx {
            data: borsh::to_vec(&tx_input).expect(
                "Failed to serialize transaction inside the batch. This is a bug, please report it",
            ),
        };

        // This closure helps us make sure that we always put the
        // `StateCheckpoint` back into `self` at the end of the function.
        let (new_checkpoint, response) = (|mut checkpoint| {
            let gas_price = self.runtime.chain_state().base_fee_per_gas(&mut checkpoint).expect("Impossible to get the gas price for the current slot. This is a bug. Please report it");

            let kernel_ws =
                KernelStateAccessor::from_checkpoint(&self.runtime.kernel(), &mut checkpoint);
            let visible_height = kernel_ws.visible_rollup_height();
            let tx_scratchpad = checkpoint.to_tx_scratchpad();

            let (tx_scratchpad, output_res) = tx_auth(
                &self.runtime,
                tx_scratchpad,
                gas_price,
                &self.sequencer_address,
                tx_input,
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
            let authz_data = &auth_output.1;
            let message = &auth_output.2;

            // If the module isn't sequencer safe, the caller must be an admin to proceed
            if !sender_is_allowed(
                &self.runtime,
                message,
                authz_data.default_address.as_ref(),
                &self.sequencer_address,
                &self.admin_addresses,
            ) {
                return (
                    tx_scratchpad.revert(),
                    Err(AcceptTxError {
                        http_status: StatusCode::FORBIDDEN.as_u16(),
                        title: "The transaction is forbidden".to_string(),
                        details: format!("Only designated admins are allowed to send `{:#?}` transactions through this sequencer", message.discriminant()),
                    }),
                );
            }

            let gas_info = gas_meter.gas_info();

            let (res, tx_scratchpad) = process_tx(
                &self.runtime,
                ValidatedAuthOutput::Valid(auth_output),
                &gas_info.gas_price,
                &gas_info.gas_used,
                &self.sequencer_address,
                visible_height,
                tx_scratchpad,
                ExecutionContext::Sequencer,
            );

            let ApplyTxResult { receipt, .. } = match res {
                Ok(x) => x,
                Err(reason) => {
                    return (
                        tx_scratchpad.revert(),
                        Err(AcceptTxError {
                            http_status: StatusCode::BAD_REQUEST.as_u16(),
                            title: "The transaction is invalid".to_string(),
                            details: format!("{:?}", reason),
                        }),
                    );
                }
            };

            match receipt.receipt {
                TxEffect::Successful(_) => {}
                TxEffect::Skipped(_) | TxEffect::Reverted(_) => {
                    return (
                        tx_scratchpad.revert(),
                        Err(AcceptTxError {
                            http_status: StatusCode::BAD_REQUEST.as_u16(),
                            title: "The transaction is invalid".to_string(),
                            details: format!("{:?}", receipt.receipt),
                        }),
                    );
                }
            }

            let tx_hash = receipt.tx_hash;
            self.txs_in_next_batch.push(TxWithHash {
                fully_baked_tx: baked_tx.clone(),
                hash: tx_hash,
            });

            (
                tx_scratchpad.commit(),
                Ok(AcceptedTx {
                    tx: baked_tx,
                    tx_hash,
                    confirmation: receipt,
                }),
            )
        })(state_checkpoint);

        self.checkpoint = Some(new_checkpoint);

        response.map(|response| {
            response.map_confirmation(|confirmation| confirmation.try_into().unwrap())
        })
    }
}

#[async_trait]
impl<Z: RtAwareBatchBuilderSpec> BatchBuilder for PreferredBatchBuilder<Z> {
    type TxInput = <Z::Rt as TransactionAuthenticator<Z::Spec>>::Input;
    type Confirmation = Confirmation<Z>;
    type Batch = PreferredBatchData;
    type Config = ();
    type Spec = Z::Spec;

    async fn create(
        storage: StorageReceiver<Z::Spec>,
        da_sync_state: Arc<DaSyncState>,
        sequencer_address: <<Z::Spec as Spec>::Da as DaSpec>::Address,
        seq_db_txs: Vec<SeqDbTx>,
        admin_addresses: Vec<<Self::Spec as Spec>::Address>,
        _config: &Self::Config,
    ) -> anyhow::Result<Self> {
        let runtime: Z::Rt = Default::default();
        let (checkpoint_sender, checkpoint_receiver) = watch::channel(StateCheckpoint::new(
            storage.borrow().clone(),
            &runtime.kernel(),
        ));
        let api_state = ApiState::build(
            Arc::new(()),
            checkpoint_receiver,
            runtime.kernel_with_slot_mapping(),
            None,
        );

        let initial_checkpoint = StateCheckpoint::new(storage.borrow().clone(), &runtime.kernel());

        let mut bb = Self {
            runtime: Default::default(),
            storage,
            sequencer_address,
            checkpoint: Some(initial_checkpoint),
            checkpoint_sender,
            api_state,
            last_da_height: da_sync_state.synced_da_height.load(Ordering::Acquire),
            da_sync_state,
            txs_in_next_batch: vec![],
            admin_addresses,
        };

        // Restore persisted transactions.
        for seq_db_tx in seq_db_txs {
            bb.accept_tx(seq_db_tx.tx_input::<Self>())
                .await
                .map_err(|err| anyhow::anyhow!("Failed to restore transactions: {:?}", err))?;
        }

        Ok(bb)
    }

    fn is_ready(&self) -> bool {
        let distance = self.da_sync_state.status().distance();
        distance <= sov_blob_storage::config_deferred_slots_count()
    }

    fn storage_receiver(&self) -> StorageReceiver<Self::Spec> {
        self.storage.clone()
    }

    fn api_state(&self) -> ApiState<Self::Spec> {
        self.api_state.clone()
    }

    fn tx_status_manager(&self) -> TxStatusManager<<Z::Spec as Spec>::Da> {
        TxStatusManager::default()
    }

    async fn set_state(&mut self, da_height: u64, stf_state: <Z::Spec as Spec>::Storage) {
        let txs_to_process = self.txs_in_next_batch.clone();

        info!(
            da_height,
            num_txs_to_process = txs_to_process.len(),
            "The sequencer is now re-applying transaction state changes on top of the latest state processed by the node"
        );

        let checkpoint = {
            let rt = Z::Rt::default();
            let kernel = rt.kernel();
            StateCheckpoint::new(stf_state, &kernel)
        };
        self.checkpoint = Some(checkpoint);

        for (idx, tx) in txs_to_process.iter().enumerate() {
            trace!(
                idx,
                tx_hash = %tx.hash,
                "Re-applying state changes for the soft-confirmed transaction"
            );

            let tx_input = borsh::from_slice(&tx.fully_baked_tx.data)
                .expect("Failed to deserialize transaction");
            if let Err(error) = self.accept_tx(tx_input).await {
                error!(
                    ?error,
                    "Transaction was soft-confirmed but failed to be re-applied"
                );
            }
        }

        self.checkpoint_sender
            .send(self.checkpoint.as_ref().unwrap().clone_with_empty_witness())
            .ok();
    }

    fn encode_tx(raw: RawTx) -> Self::TxInput {
        Z::Rt::add_standard_auth(raw)
    }

    async fn accept_tx(
        &mut self,
        tx_input: Self::TxInput,
    ) -> Result<AcceptedTx<Self::Confirmation>, AcceptTxError> {
        let response = self.accept_tx_internal(tx_input).await;

        self.checkpoint_sender
            .send(
                self.checkpoint
                    .as_ref()
                    .expect("Missing internal checkpoint; this is a bug, please report it")
                    .clone_with_empty_witness(),
            )
            .ok();

        response
    }

    async fn build_next_batch(&mut self, height: u64) -> anyhow::Result<FreshlyBuiltBatch<Self>> {
        let (txs, hashes) = self
            .txs_in_next_batch
            .iter()
            .map(|tx| (tx.fully_baked_tx.clone(), tx.hash))
            .unzip();

        let batch = FreshlyBuiltBatch {
            inner: PreferredBatchData {
                data: Batch { txs },
                virtual_slots_to_advance: height
                    .saturating_sub(self.last_da_height)
                    .min(1)
                    .try_into()
                    .unwrap_or(u8::MAX),
                sequence_number: 0,
            },
            hashes,
        };

        self.last_da_height = height;
        Ok(batch)
    }

    async fn clear_batch(&mut self) -> anyhow::Result<()> {
        self.txs_in_next_batch.clear();
        Ok(())
    }
}
