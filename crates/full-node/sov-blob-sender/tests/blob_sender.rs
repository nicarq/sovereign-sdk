use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sov_blob_sender::{BlobSender, BlobSenderHooks, FinalizationManager};
use sov_mock_da::storable::layer::StorableMockDaLayer;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_rollup_interface::da::BlobReaderTrait;
use sov_rollup_interface::node::da::DaService;
use tokio::sync::{watch, RwLock};
use tokio::time::sleep;
struct TestHooks {}

impl BlobSenderHooks for TestHooks {
    type Da = MockDaSpec;
}

#[derive(Clone)]
struct TestFinalizationManager {}

#[async_trait]
impl FinalizationManager for TestFinalizationManager {
    async fn is_blob_finalized(&self, _blob_hash: [u8; 32]) -> anyhow::Result<Option<bool>> {
        Ok(Some(true))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn blob_sender_posts_data_to_da() {
    let da_path = tempfile::tempdir().unwrap();
    let da_layer = Arc::new(RwLock::new(
        StorableMockDaLayer::new_in_path(da_path.path(), 0)
            .await
            .unwrap(),
    ));
    let da = StorableMockDaService::new_manual_producing(MockAddress::new([0; 32]), da_layer).await;

    let finalization_manager = TestFinalizationManager {};
    let storage_path = tempfile::tempdir().unwrap();
    let (_sender, shutdown_receiver) = watch::channel(());

    let hooks = TestHooks {};

    let (mut blob_sender, _handle) = BlobSender::new(
        da.clone(),
        finalization_manager,
        storage_path.path(),
        true,
        hooks,
        shutdown_receiver,
    )
    .await
    .unwrap();

    let data = Arc::new([1u8, 2, 3, 4, 5]);
    blob_sender
        .publish_batch_blob(data.clone(), 0)
        .await
        .unwrap();

    sleep(Duration::from_secs(1)).await;
    da.produce_block_now().await.unwrap();

    let mut binding = da.get_block_at(1).await.unwrap();
    let data_from_da = binding.batch_blobs[0].full_data();
    assert_eq!(data.as_slice(), data_from_da);
}
