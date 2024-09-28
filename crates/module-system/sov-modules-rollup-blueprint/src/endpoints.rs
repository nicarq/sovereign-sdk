use sov_db::ledger_db::LedgerDb;
use sov_ledger_apis::LedgerRoutes;
use sov_modules_api::execution_mode::ExecutionMode;
use sov_modules_api::hooks::ApplyBatchHooks;
use sov_modules_api::rest::StorageReceiver;
use sov_modules_api::{RuntimeEventProcessor, Spec};
use sov_modules_stf_blueprint::{Runtime as RuntimeTrait, RuntimeEndpoints, TxReceiptContents};
use sov_rollup_interface::zk::{ZkvmGuest, ZkvmHost};
use sov_sequencer::batch_builders::standard::StdBatchBuilder;
use sov_sequencer::batch_builders::BatchBuilder;
use sov_sequencer::{BatchBuilderConfig, SequencerConfig, SequencerDb};

use crate::{FullNodeBlueprint, SequencerBlueprint};

/// Register rollup's default RPC methods and Axum router.
pub async fn register_endpoints<B, M>(
    storage: StorageReceiver<B::Spec>,
    ledger_db: &LedgerDb,
    sequencer_db: &SequencerDb,
    da_service: &B::DaService,
    sequencer_config: &SequencerConfig<B::DaSpec>,
) -> anyhow::Result<RuntimeEndpoints>
where
    B: FullNodeBlueprint<M> + 'static,
    M: ExecutionMode + 'static,
    B::Runtime: RuntimeEventProcessor,
    <B::InnerZkvmHost as ZkvmHost>::Guest: ZkvmGuest<Verifier = <B::Spec as Spec>::InnerZkvm>,
    <B::OuterZkvmHost as ZkvmHost>::Guest: ZkvmGuest<Verifier = <B::Spec as Spec>::OuterZkvm>,
{
    let da_address = sequencer_config.da_address.clone();
    let (api_state, sequencer_router) = match &sequencer_config.batch_builder {
        BatchBuilderConfig::Standard(bb_config) => {
            let batch_builder =
                StdBatchBuilder::<(B::Spec, B::DaSpec, B::Runtime), B::Kernel>::create(
                    storage,
                    da_address,
                    sequencer_db.read_all()?,
                    bb_config,
                )
                .await?;
            let tx_status_manager = batch_builder.tx_status_manager();
            let sequencer = SequencerBlueprint::<B, M>::new(
                batch_builder,
                da_service.clone(),
                tx_status_manager,
                sequencer_db.clone(),
                ledger_db.clone(),
            );

            (
                sequencer.api_state(),
                sequencer.rest_api_server("/sequencer"),
            )
        }
        BatchBuilderConfig::Preferred => {
            todo!("Preferred sequencer is not yet supported")
        }
    };

    let mut endpoints = B::Runtime::endpoints(api_state);

    // Sequencer endpoints.
    endpoints.axum_router = endpoints.axum_router.nest("/sequencer", sequencer_router);

    // Ledger endpoint.
    {
        let ledger_axum_router = LedgerRoutes::<
            LedgerDb,
            // Can keep hard-coding:
            // BatchSequencerReceipt<B::DaSpec>,
            // or use some associated type.
            // TODO: But ideally it needs to be addressed properly: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1268
            <B::Runtime as ApplyBatchHooks<B::DaSpec>>::BatchResult,
            TxReceiptContents<B::Spec>,
            <B::Runtime as RuntimeEventProcessor>::RuntimeEvent,
        >::axum_router(ledger_db.clone(), "/ledger");
        endpoints.axum_router = endpoints
            .axum_router
            .nest("/ledger", ledger_axum_router.with_state(ledger_db.clone()));
    }

    Ok(endpoints)
}
