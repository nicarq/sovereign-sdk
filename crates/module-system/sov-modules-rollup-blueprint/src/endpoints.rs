use sov_db::ledger_db::LedgerDb;
use sov_ledger_apis::LedgerRoutes;
use sov_modules_api::capabilities::Authenticator;
use sov_modules_api::execution_mode::ExecutionMode;
use sov_modules_api::hooks::ApplyBatchHooks;
use sov_modules_api::{RuntimeEventProcessor, Spec};
use sov_modules_stf_blueprint::{Runtime as RuntimeTrait, RuntimeEndpoints, TxReceiptContents};
use sov_rollup_interface::zk::{ZkvmGuest, ZkvmHost};
use sov_sequencer::{FairBatchBuilder, FairBatchBuilderConfig, SequencerDb, TxStatusManager};
use sov_stf_runner::SequencerConfig;
use tokio::sync::watch;

use crate::{FullNodeBlueprint, SequencerBlueprint};

/// Register rollup's default RPC methods and Axum router.
pub fn register_endpoints<B, M, Auth>(
    storage: watch::Receiver<<B::Spec as Spec>::Storage>,
    ledger_db: &LedgerDb,
    sequencer_db: &SequencerDb,
    da_service: &B::DaService,
    sequencer_config: &SequencerConfig<FairBatchBuilderConfig<B::DaSpec>>,
) -> anyhow::Result<RuntimeEndpoints>
where
    B: FullNodeBlueprint<M> + 'static,
    M: ExecutionMode + 'static,
    B::Runtime: RuntimeEventProcessor,
    Auth: Authenticator<Spec = B::Spec, DispatchCall = B::Runtime>,
    <B::InnerZkvmHost as ZkvmHost>::Guest: ZkvmGuest<Verifier = <B::Spec as Spec>::InnerZkvm>,
    <B::OuterZkvmHost as ZkvmHost>::Guest: ZkvmGuest<Verifier = <B::Spec as Spec>::OuterZkvm>,
{
    let mut endpoints = B::Runtime::endpoints(storage.clone());

    // Ledger endpoint.
    {
        let ledger_axum_router = LedgerRoutes::<
            LedgerDb,
            // Can keep hard-coding:
            // BatchSequencerReceipt<B::DaSpec>,
            // or use some associated type.
            // TODO: But ideally it needs to be addressed properly: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1268
            <B::Runtime as ApplyBatchHooks<B::DaSpec>>::BatchResult,
            TxReceiptContents,
            <B::Runtime as RuntimeEventProcessor>::RuntimeEvent,
        >::axum_router(ledger_db.clone(), "/ledger");
        endpoints.axum_router = endpoints
            .axum_router
            .nest("/ledger", ledger_axum_router.with_state(ledger_db.clone()));
    }

    // Sequencer endpoints.
    {
        let tx_status_manager = TxStatusManager::default();
        let batch_builder =
            FairBatchBuilder::<B::Spec, B::DaSpec, B::Runtime, B::Kernel, Auth>::new(
                B::Runtime::default(),
                B::Kernel::default(),
                tx_status_manager.clone(),
                storage,
                sequencer_db.clone(),
                sequencer_config.batch_builder.clone(),
            )?;

        let sequencer = SequencerBlueprint::<B, M, Auth>::new(
            batch_builder,
            da_service.clone(),
            tx_status_manager,
            ledger_db.clone(),
        );

        endpoints.axum_router = endpoints
            .axum_router
            .nest("/sequencer", sequencer.rest_api_server("/sequencer"));
    }

    Ok(endpoints)
}
