use anyhow::Context as _;
use sov_db::ledger_db::LedgerDb;
use sov_modules_api::{RuntimeEventDisplay, Spec};
use sov_modules_stf_blueprint::{Runtime as RuntimeTrait, SequencerOutcome, TxEffect};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::services::da::DaService;
use sov_sequencer::{FairBatchBuilder, FairBatchBuilderConfig, Sequencer, SequencerDb};
use tokio::sync::watch;

use crate::RollupBlueprint;

/// Register rollup's default RPC methods and Axum router.
pub fn register_endpoints<B>(
    storage: watch::Receiver<<B::NativeSpec as Spec>::Storage>,
    ledger_db: &LedgerDb,
    sequencer_db: &SequencerDb,
    da_service: &B::DaService,
    sequencer: <B::DaSpec as DaSpec>::Address,
) -> anyhow::Result<(jsonrpsee::RpcModule<()>, axum::Router<()>)>
where
    B: RollupBlueprint + 'static,
    B::NativeRuntime: RuntimeEventDisplay,
    <B::DaService as DaService>::TransactionId: Clone + Send + Sync + serde::Serialize,
{
    let mut axum_router = axum::Router::<()>::new();

    // Runtime endpoints.
    let mut rpc_methods = B::NativeRuntime::rpc_methods(storage.clone());

    // Ledger endpoint.
    {
        rpc_methods.merge(sov_ledger_apis::server::rpc_module::<
            LedgerDb,
            SequencerOutcome,
            TxEffect,
            <B::NativeRuntime as sov_modules_api::RuntimeEventDisplay>::RuntimeEvent,
        >(ledger_db.clone())?)?;
    }

    // Sequencer endpoints.
    {
        let config = FairBatchBuilderConfig {
            mempool_max_txs_count: u32::MAX as usize,
            max_batch_size_bytes: 1024 * 100,
            sequencer_address: sequencer.clone(),
        };
        let batch_builder =
            FairBatchBuilder::<B::NativeSpec, B::DaSpec, B::NativeRuntime, B::NativeKernel>::new(
                B::NativeRuntime::default(),
                B::NativeKernel::default(),
                storage,
                sequencer_db.clone(),
                config,
            )?;

        let sequencer = Sequencer::new(batch_builder, da_service.clone());

        rpc_methods
            .merge(sequencer.rpc())
            .context("Failed to merge Transactions RPC modules")?;

        axum_router = axum_router.nest(
            "/sequencer",
            sequencer.axum_router("/sequencer").with_state(sequencer),
        );
    }

    Ok((rpc_methods, axum_router))
}
