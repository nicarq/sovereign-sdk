use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use jsonrpsee::core::client::SubscriptionClientT;
use sov_bank::utils::TokenHolder;
use sov_bank::{Coins, TokenId};
use sov_db::ledger_db::{LedgerDb, SlotCommit};
use sov_db::schema::{CacheContainer, CacheDb};
use sov_ledger_apis::rest::LedgerRoutes;
use sov_ledger_apis::rpc::client::RpcClient;
use sov_ledger_apis::rpc::server::rpc_module;
use sov_mock_da::{MockBlock, MockDaSpec};
use sov_modules_api::{
    AggregatedProofPublicData, CodeCommitment, ModuleId, RuntimeEventResponse, StoredEvent,
};
use sov_modules_stf_blueprint::BatchReceipt;
use sov_rollup_interface::rpc::{BatchResponse, SlotResponse, TxResponse};
use sov_rollup_interface::stf::TransactionReceipt;
use sov_rollup_interface::zk::aggregated_proof::{AggregatedProof, SerializedAggregatedProof};
use tempfile::{tempdir, TempDir};

use crate::TestSpec;

type TestEvent = demo_stf::runtime::RuntimeEvent<TestSpec, MockDaSpec>;

pub extern crate sov_ledger_json_client;

/// Very, very simple utility function: it just persists some dummy data to the
/// [`LedgerDb`], so that it's not empty when you read it within tests.
pub async fn add_data_to_ledger_db(ledger_db: &LedgerDb) -> anyhow::Result<()> {
    let block_a = MockBlock::default();

    let mut slot: SlotCommit<MockBlock, i32, i32> = SlotCommit::new(block_a);

    let tx_receipts = vec![TransactionReceipt {
        tx_hash: [1; 32],
        body_to_save: Some(b"tx-body".to_vec()),
        events: events(),
        receipt: 0,
        gas_used: vec![0, 1, u64::MAX],
    }];

    slot.add_batch(BatchReceipt {
        batch_hash: [10; 32],
        tx_receipts,
        inner: 0,
        gas_price: vec![0, 1, u64::MAX],
    });

    ledger_db.commit_slot(slot, b"state-root")?;

    ledger_db.save_finalized_aggregated_proof(AggregatedProof::new(
        SerializedAggregatedProof {
            raw_aggregated_proof: b"aggregated-proof".to_vec(),
        },
        // By filling all the fields, clients can more thoroughly test
        // (de)serialization logic.
        //
        // This data doesn't make any sense (they're not even hashes...), but
        // it's just for testing.
        AggregatedProofPublicData {
            validity_conditions: vec![],
            initial_slot_number: u64::MAX,
            final_slot_number: u64::MAX,
            genesis_state_root: b"genesis-state-root".to_vec(),
            initial_state_root: b"initial-state-root".to_vec(),
            final_state_root: b"final-state-root".to_vec(),
            initial_slot_hash: b"initial-slot-hash".to_vec(),
            final_slot_hash: b"final-slot-hash".to_vec(),
            code_commitment: CodeCommitment(b"code-commitment".to_vec()),
        },
    ))?;

    Ok(())
}

fn events() -> Vec<StoredEvent> {
    let holder = TokenHolder::Module(ModuleId::from([0; 32]));
    let token_id =
        TokenId::from_str("token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6")
            .unwrap();

    let event_value1 = TestEvent::bank(sov_bank::event::Event::TokenCreated {
        token_name: "token".to_string(),
        coins: Coins {
            amount: 0,
            token_id,
        },
        minter: holder.clone(),
        authorized_minters: vec![],
    });
    let event_value2 = TestEvent::bank(sov_bank::event::Event::TokenFrozen {
        token_id,
        freezer: holder,
    });

    vec![
        StoredEvent::new("foo".as_bytes(), &borsh::to_vec(&event_value1).unwrap()),
        StoredEvent::new("bar".as_bytes(), &borsh::to_vec(&event_value2).unwrap()),
    ]
}

/// Everything that one needs to run tests against the ledger APIs.
pub struct LedgerTestService {
    // Must be kept in scope during the test to avoid directory deletion.
    _dir: TempDir,
    pub rpc_handle: jsonrpsee::server::ServerHandle,
    pub rpc_addr: SocketAddr,
    pub axum_handle: axum_server::Handle,
    pub axum_client: sov_ledger_json_client::Client,
}

impl LedgerTestService {
    /// Instantiates a new [`LedgerDb`] and starts serving data over both JSON-RPC and Axum.
    pub async fn new() -> anyhow::Result<LedgerTestService> {
        let dir = tempdir()?;
        let schema_db = LedgerDb::get_rockbound_options().default_setup_db_in_path(dir.path())?;
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
            _dir: dir,
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

#[cfg(test)]
mod tests {
    use sov_mock_da::MockDaSpec;

    use super::*;
    use crate::TestSpec;

    #[test]
    fn events_deserialize_correctly() {
        let events = events();
        for event in events {
            <demo_stf::runtime::RuntimeEvent<TestSpec, MockDaSpec> as borsh::BorshDeserialize>::deserialize(
                &mut &event.value().inner()[..]).unwrap();
        }
    }
}
