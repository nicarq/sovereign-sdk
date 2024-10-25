use std::net::SocketAddr;
use std::sync::Arc;

use sov_api_spec::Client;
use sov_modules_api::prelude::tokio::sync::watch;
use sov_rollup_apis::{DefaultRollupStateProvider, RollupTxRouter};
use sov_test_utils::{generate_optimistic_runtime, TestUser};
mod rest_api;
use sov_modules_api::prelude::tokio;
use sov_modules_api::{Spec, SyncStatus};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;

type S = sov_test_utils::TestSpec;

generate_optimistic_runtime!(TestRuntime <= );

type RT = TestRuntime<S>;

struct TestData {
    runner: TestRunner<RT, S>,

    /// A channel to send the storage over. This should be subscribed to the same channel as [`Self::rollup_tx_router`].
    storage_sender: watch::Sender<<S as Spec>::Storage>,

    user: TestUser<S>,

    axum_addr: SocketAddr,
    axum_server: axum_server::Handle,

    sync_sender: watch::Sender<SyncStatus>,
}

impl Drop for TestData {
    fn drop(&mut self) {
        self.axum_server.shutdown();
    }
}

impl TestData {
    pub async fn setup() -> Self {
        // Generate a genesis config, then overwrite the attester key/address with ones that
        // we know. We leave the other values untouched.
        let genesis_config =
            HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

        let sequencer_da_address = genesis_config.initial_sequencer.da_address;

        let user = genesis_config.additional_accounts[0].clone();

        let runtime = RT::default();

        // Run genesis registering the attester and sequencer we've generated.
        let genesis_config = GenesisConfig::from_minimal_config(genesis_config.into());

        let runner = TestRunner::new_with_genesis(genesis_config.into_genesis_params(), runtime);

        let storage = runner.storage_manager().create_storage();

        let (storage_sender, storage_receiver) = watch::channel(storage);
        let (sync_sender, sync_receiver) = watch::channel(SyncStatus::Syncing {
            synced_da_height: 0,
            target_da_height: 0,
        });

        let axum_router: axum::Router<()> =
            RollupTxRouter::<Arc<DefaultRollupStateProvider<S, RT>>>::axum_router(
                storage_receiver,
                sequencer_da_address,
                sync_receiver,
            );

        let (axum_addr, axum_server) = {
            let handle = axum_server::Handle::new();

            let handle1 = handle.clone();
            tokio::spawn(async move {
                axum_server::Server::bind(SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)))
                    .handle(handle1)
                    .serve(axum_router.into_make_service())
                    .await
                    .unwrap();
            });

            (handle.listening().await.unwrap(), handle)
        };

        TestData {
            runner,
            storage_sender,
            user,
            axum_addr,
            axum_server,
            sync_sender,
        }
    }

    /// Sends the current storage over the [`Self::storage_sender`] channel to update the [`Self::rollup_tx_router`].
    /// Remove the dead code warning once <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1552> is fixed.
    #[allow(dead_code)]
    pub fn send_storage(&self) {
        assert!(
            !self.storage_sender.is_closed(),
            "The storage sender channel is closed"
        );

        let storage = self.runner.storage_manager().create_storage();
        self.storage_sender.send_replace(storage);
    }

    pub fn send_sync_status(&self, status: SyncStatus) {
        self.sync_sender.send(status).unwrap();
    }

    /// Returns a [`Client`] REST handler for the sequencer.
    pub fn client(&self) -> Client {
        Client::new(&format!("http://{}", self.axum_addr))
    }
}
