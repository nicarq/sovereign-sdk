#![allow(dead_code)]

use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use jsonrpsee::core::client::SubscriptionClientT;
use sov_db::ledger_db::LedgerDb;
use sov_db::schema::{CacheContainer, CacheDb};
use sov_ledger_apis::jsonapi::LedgerRoutes;
use sov_ledger_apis::rpc::client::RpcClient;
use sov_ledger_apis::rpc::server::rpc_module;
use sov_mock_da::MockDaSpec;
use sov_modules_api::RuntimeEventResponse;
use sov_rollup_interface::rpc::{BatchResponse, SlotResponse, TxResponse};
use sov_test_utils::ledger_db::add_data_to_ledger_db;
use sov_test_utils::TestSpec;
use tempfile::{tempdir, TempDir};

type TestEvent = demo_stf::runtime::RuntimeEvent<TestSpec, MockDaSpec>;

/// Everything that one needs to run tests against the ledger APIs.
pub struct LedgerTestService {
    // Must be kept in scope during the test to avoid directory deletion.
    dir: TempDir,
    pub rpc_handle: jsonrpsee::server::ServerHandle,
    pub rpc_addr: SocketAddr,
    pub axum_handle: axum_server::Handle,
    pub axum_client: sov_ledger_json_client::Client,
}

impl LedgerTestService {
    /// Instantiates a new [`LedgerDb`] and starts serving data over both JSON-RPC and Axum.
    pub async fn new() -> anyhow::Result<LedgerTestService> {
        let dir = tempdir()?;
        let schema_db = LedgerDb::setup_schema_db(dir.path())?;
        let cache_container =
            CacheContainer::new(schema_db, Arc::new(RwLock::new(Default::default())).into());
        let cache_db = CacheDb::new(0, Arc::new(RwLock::new(cache_container)).into());
        let ledger_db = LedgerDb::with_cache_db(cache_db)?;

        add_data_to_ledger_db(&ledger_db).await?;

        let rpc_module =
            rpc_module::<LedgerDb, u32, u32, RuntimeEventResponse<TestEvent>>(ledger_db.clone())?;

        let server = jsonrpsee::server::ServerBuilder::default()
            .build("127.0.0.1:0")
            .await?;
        let rpc_addr = server.local_addr()?;

        let axum_handle = axum_server::Handle::new();
        let axum_handle1 = axum_handle.clone();
        let ledger_db1 = ledger_db.clone();
        tokio::spawn(async move {
            let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
            axum_server::Server::bind(addr)
                .handle(axum_handle1)
                .serve(
                    LedgerRoutes::<LedgerDb, u32, u32, TestEvent>::axum_router(
                        ledger_db1.clone(),
                        "/ledger",
                    )
                    .with_state::<()>(ledger_db1)
                    .into_make_service(),
                )
                .await
                .unwrap();
        });

        let axum_addr = axum_handle
            .listening()
            .await
            .ok_or(anyhow::anyhow!("Failed to bind"))?;
        let axum_client = sov_ledger_json_client::Client::new(&format!("http://{}", axum_addr));

        Ok(Self {
            dir,
            rpc_handle: server.start(rpc_module),
            rpc_addr,
            axum_handle,
            axum_client,
        })
    }

    pub async fn rpc_client(
        &self,
    ) -> Arc<
        impl RpcClient<SlotResponse<u32, u32>, BatchResponse<u32, u32>, TxResponse<u32>>
            + SubscriptionClientT,
    > {
        Arc::new(
            jsonrpsee::ws_client::WsClientBuilder::new()
                .build(format!("ws://{}", self.rpc_addr))
                .await
                .unwrap(),
        )
    }
}
