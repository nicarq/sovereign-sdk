use anyhow::Context as _;
use sov_db::ledger_db::LedgerDb;
use sov_ledger_apis::jsonapi::LedgerRoutes;
use sov_modules_api::{Authenticator, RuntimeEventProcessor, RuntimeEventResponse, Spec};
use sov_modules_stf_blueprint::{BatchSequencerOutcome, Runtime as RuntimeTrait, TxEffect};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::services::da::DaService;
use sov_sequencer::{FairBatchBuilder, FairBatchBuilderConfig, Sequencer, SequencerDb};
use tokio::sync::watch;

use crate::RollupBlueprint;

/// Register rollup's default RPC methods and Axum router.
pub fn register_endpoints<B, Auth>(
    storage: watch::Receiver<<B::NativeSpec as Spec>::Storage>,
    ledger_db: &LedgerDb,
    sequencer_db: &SequencerDb,
    da_service: &B::DaService,
    sequencer: <B::DaSpec as DaSpec>::Address,
) -> anyhow::Result<(jsonrpsee::RpcModule<()>, axum::Router<()>)>
where
    B: RollupBlueprint + 'static,
    B::NativeRuntime: RuntimeEventProcessor,
    <B::DaService as DaService>::TransactionId: Clone + Send + Sync + serde::Serialize,
    Auth: Authenticator<Spec = B::NativeSpec, DispatchCall = B::NativeRuntime>,
{
    let mut axum_router = axum::Router::<()>::new();

    // Runtime endpoints.
    let mut rpc_methods = B::NativeRuntime::rpc_methods(storage.clone());

    // Ledger endpoint.
    {
        rpc_methods.merge(sov_ledger_apis::rpc::server::rpc_module::<
            LedgerDb,
            BatchSequencerOutcome,
            TxEffect,
            RuntimeEventResponse<<B::NativeRuntime as RuntimeEventProcessor>::RuntimeEvent>,
        >(ledger_db.clone())?)?;

        let ledger_axum_router = LedgerRoutes::<
            LedgerDb,
            BatchSequencerOutcome,
            TxEffect,
            <B::NativeRuntime as RuntimeEventProcessor>::RuntimeEvent,
        >::axum_router(ledger_db.clone(), "/ledger");
        axum_router = axum_router.nest("/ledger", ledger_axum_router.with_state(ledger_db.clone()));
    }

    // Sequencer endpoints.
    {
        let config = FairBatchBuilderConfig {
            mempool_max_txs_count: u32::MAX as usize,
            max_batch_size_bytes: 1024 * 100,
            sequencer_address: sequencer.clone(),
        };
        let batch_builder = FairBatchBuilder::<
            B::NativeSpec,
            B::DaSpec,
            B::NativeRuntime,
            B::NativeKernel,
            Auth,
        >::new(
            B::NativeRuntime::default(),
            B::NativeKernel::default(),
            storage,
            sequencer_db.clone(),
            config,
        )?;

        let sequencer = Sequencer::<_, _, Auth>::new(batch_builder, da_service.clone());

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
