use anyhow::Context as _;
use sov_db::ledger_db::LedgerDb;
use sov_modules_api::runtime::capabilities::Kernel;
use sov_modules_api::Spec;
use sov_modules_stf_blueprint::{Runtime as RuntimeTrait, SequencerOutcome, TxEffect};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::services::da::DaService;
use sov_sequencer::{FairBatchBuilder, Sequencer, SequencerDb};
use tokio::sync::watch;

/// Register rollup's default rpc methods.
pub fn register_rpc<RT, K, S, Da>(
    storage: watch::Receiver<S::Storage>,
    ledger_db: &LedgerDb,
    sequencer_db: &SequencerDb,
    da_service: &Da,
    sequencer: <Da::Spec as DaSpec>::Address,
) -> Result<jsonrpsee::RpcModule<()>, anyhow::Error>
where
    RT: RuntimeTrait<S, <Da as DaService>::Spec> + 'static + sov_modules_api::RuntimeEventDisplay,
    K: Kernel<S, Da::Spec> + 'static,
    S: Spec,
    Da: DaService + Clone,
    Da::TransactionId: Clone + serde::Serialize + Send + Sync,
{
    // runtime RPC.
    let mut rpc_methods = RT::rpc_methods(storage.clone());

    // ledger RPC.
    {
        rpc_methods.merge(sov_ledger_rpc::server::rpc_module::<
            LedgerDb,
            SequencerOutcome,
            TxEffect,
            <RT as sov_modules_api::RuntimeEventDisplay>::RuntimeEvent,
        >(ledger_db.clone())?)?;
    }

    // sequencer RPC.
    {
        let batch_builder = FairBatchBuilder::<S, Da::Spec, RT, K>::new(
            1024 * 100,
            u32::MAX as usize,
            RT::default(),
            K::default(),
            storage,
            sequencer,
            sequencer_db.clone(),
        )?;

        let sequencer_rpc = Sequencer::new(batch_builder, da_service.clone()).rpc();
        rpc_methods
            .merge(sequencer_rpc)
            .context("Failed to merge Transactions RPC modules")?;
    }

    Ok(rpc_methods)
}
