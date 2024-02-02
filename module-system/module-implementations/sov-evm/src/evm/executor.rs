use std::convert::Infallible;

use reth_primitives::TransactionSignedEcRecovered;
use revm::primitives::{CfgEnv, EVMError, Env, ExecutionResult};
use revm::{self, Database, DatabaseCommit};

use super::conversions::create_tx_env;
use super::primitive_types::BlockEnv;

pub(crate) fn execute_tx<DB: Database<Error = Infallible> + DatabaseCommit>(
    db: DB,
    block_env: &BlockEnv,
    tx: &TransactionSignedEcRecovered,
    config_env: CfgEnv,
) -> Result<ExecutionResult, EVMError<Infallible>> {
    let mut evm = revm::new();

    let env = Env {
        block: block_env.into(),
        cfg: config_env,
        tx: create_tx_env(tx),
    };

    evm.env = env;
    evm.database(db);
    evm.transact_commit()
}

#[cfg(feature = "native")]
pub(crate) fn inspect<DB: Database<Error = Infallible> + DatabaseCommit>(
    db: DB,
    block_env: &BlockEnv,
    tx: revm::primitives::TxEnv,
    config_env: CfgEnv,
) -> Result<revm::primitives::ResultAndState, EVMError<Infallible>> {
    let mut evm = revm::new();

    let env = Env {
        cfg: config_env,
        block: block_env.into(),
        tx,
    };

    evm.env = env;
    evm.database(db);

    let config = reth_revm_inspectors::tracing::TracingInspectorConfig::all();

    let mut inspector = reth_revm_inspectors::tracing::TracingInspector::new(config);

    evm.inspect(&mut inspector)
}
