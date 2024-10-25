use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use base64::prelude::*;
use sov_api_spec::types;
use sov_mock_da::MockDaService;
use sov_rollup_interface::node::{DaSyncState, SyncStatus};
use sov_sequencer::batch_builders::preferred::PreferredBatchBuilder;
use sov_sequencer::batch_builders::standard::{StdBatchBuilder, StdBatchBuilderConfig};
use sov_sequencer::batch_builders::BatchBuilder;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestOptimisticRuntime;
use sov_test_utils::sequencer::TestSequencerSetup;
use sov_test_utils::TestSpec;
use tokio::sync::watch;

use crate::utils::{generate_paymaster_tx, generate_txs};

#[tokio::test(flavor = "multi_thread")]
async fn restore_txs_from_seq_db() {
    let dir = tempfile::tempdir().unwrap();
    let sequencer_addr = HighLevelOptimisticGenesisConfig::SEQUENCER_DA_ADDR;
    let da_service = MockDaService::new(sequencer_addr);

    let batch_builder_config = StdBatchBuilderConfig {
        mempool_max_txs_count: None,
        max_batch_size_bytes: None,
    };

    let sequencer = TestSequencerSetup::<
        StdBatchBuilder<(TestSpec, TestOptimisticRuntime<TestSpec>)>,
    >::new(dir, da_service, batch_builder_config, vec![], true)
    .await
    .unwrap();

    let tx = generate_txs(sequencer.admin_private_key.clone())[0].clone();
    {
        let client = sequencer.client();

        client
            .accept_tx(&types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx.raw_tx),
            })
            .await
            .unwrap();
    }

    let seq_db = sequencer.sequencer.db().clone();

    let db_txs = seq_db.read_all().unwrap();
    assert_eq!(db_txs.len(), 1);
    assert_eq!(db_txs[0].fully_baked_tx(), tx.fully_baked_tx);

    let (sync_status_sender, _) = watch::channel(SyncStatus::Syncing {
        synced_da_height: 0,
        target_da_height: 0,
    });

    let da_sync_state = Arc::new(DaSyncState {
        synced_da_height: AtomicU64::new(0),
        target_da_height: AtomicU64::new(0),
        sync_status_sender,
    });

    let mut restored_batch_builder: PreferredBatchBuilder<(
        TestSpec,
        TestOptimisticRuntime<TestSpec>,
    )> = PreferredBatchBuilder::create(
        sequencer.sequencer.batch_builder().await.storage_receiver(),
        da_sync_state,
        sequencer_addr,
        db_txs,
        Vec::new(),
        &(),
    )
    .await
    .unwrap();

    let batch = restored_batch_builder.build_next_batch(0).await.unwrap();

    assert_eq!(batch.hashes.len(), 1);
}

// Checks that transactions that are not sequencer safe are rejected
// when the sender address is not configured as an admin in the sequencer config.
#[tokio::test(flavor = "multi_thread")]
async fn not_sequencer_safe_txs_are_restricted() {
    let dir = tempfile::tempdir().unwrap();
    let sequencer_addr = HighLevelOptimisticGenesisConfig::SEQUENCER_DA_ADDR;
    let da_service = MockDaService::new(sequencer_addr);

    let sequencer = TestSequencerSetup::<
        PreferredBatchBuilder<(TestSpec, TestOptimisticRuntime<TestSpec>)>,
    >::new(dir, da_service, (), vec![], false)
    .await
    .unwrap();

    let tx = generate_paymaster_tx(sequencer.admin_private_key.clone());
    {
        let client = sequencer.client();

        if let Err(e) = client
            .accept_tx(&types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
        {
            assert!(
                e.to_string().contains("Only designated admins are allowed"),
                "Unexpected error: {}",
                e
            );
        } else {
            panic!("Sequencer accepted admin tx from non-admin sender");
        }
    }
}

// Checks that transactions that are not sequencer safe are accepted
// if the sender address is configured as an admin in the sequencer config.
#[tokio::test(flavor = "multi_thread")]
async fn sequencer_safe_txs_from_admins_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let sequencer_addr = HighLevelOptimisticGenesisConfig::SEQUENCER_DA_ADDR;
    let da_service = MockDaService::new(sequencer_addr);

    let sequencer = TestSequencerSetup::<
        PreferredBatchBuilder<(TestSpec, TestOptimisticRuntime<TestSpec>)>,
    >::new(dir, da_service, (), vec![], true)
    .await
    .unwrap();

    let tx = generate_paymaster_tx(sequencer.admin_private_key.clone());
    {
        let client = sequencer.client();

        client
            .accept_tx(&types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await.expect("Transactions from sequencer admins should be accepted even if they are sequencer unsafe");
    }
}
