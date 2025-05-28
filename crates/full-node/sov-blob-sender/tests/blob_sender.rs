use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sov_blob_sender::{BlobInternalId, BlobSender, BlobSenderHooks, FinalizationManager};
use sov_mock_da::storable::layer::StorableMockDaLayer;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::da::BlockHeaderTrait;
use sov_rollup_interface::da::BlobReaderTrait;
use sov_rollup_interface::node::da::DaService;
use tempfile::TempDir;
use tokio::sync::{watch, RwLock};
use tokio::time::sleep;

struct TestHooks {}

#[async_trait]
impl BlobSenderHooks for TestHooks {
    type Da = MockDaSpec;
}

#[derive(Clone)]
struct TestFinalizationManager<Da: DaService> {
    da: Da,
    start_da_height: u64,
}

impl<Da: DaService> TestFinalizationManager<Da> {
    async fn is_blob_posted_on_da(
        &self,
        blob_id: BlobInternalId,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<bool>> {
        for height in start..(end + 1) {
            let data = data_at(&self.da, height).await;
            for d in data {
                if !d.is_empty() && d[0] as BlobInternalId == blob_id {
                    return Ok(Some(true));
                }
            }
        }

        Ok(None)
    }
}

#[async_trait]
impl<Da> FinalizationManager for TestFinalizationManager<Da>
where
    Da: DaService<Error = anyhow::Error>,
{
    async fn is_blob_finalized(
        &self,
        _blob_hash: [u8; 32],
        blob_id: BlobInternalId,
    ) -> anyhow::Result<Option<bool>> {
        let last_finalized_block_number = self.da.get_last_finalized_block_number().await?;

        let is_finalized = self
            .is_blob_posted_on_da(blob_id, self.start_da_height, last_finalized_block_number)
            .await?;

        match is_finalized {
            Some(_) => Ok(is_finalized),
            None => {
                let header = self.da.get_head_block_header().await?;
                self.is_blob_posted_on_da(blob_id, last_finalized_block_number + 1, header.height())
                    .await
            }
        }
    }
}

async fn create_da() -> (StorableMockDaService, TempDir) {
    let da_dir = tempfile::tempdir().unwrap();
    let da_layer = Arc::new(RwLock::new(
        StorableMockDaLayer::new_in_path(da_dir.path(), 0)
            .await
            .unwrap(),
    ));
    (
        StorableMockDaService::new_manual_producing(MockAddress::new([0; 32]), da_layer).await,
        da_dir,
    )
}

async fn create_blob_sender(
    da: StorableMockDaService,
    shutdown_sender: watch::Sender<()>,
) -> BlobSender<StorableMockDaService, TestHooks, TestFinalizationManager<StorableMockDaService>> {
    let finalization_manager = TestFinalizationManager {
        da: da.clone(),
        start_da_height: 0,
    };

    let storage_path = tempfile::tempdir().unwrap();

    let hooks = TestHooks {};

    let (blob_sender, _handle) = BlobSender::new_with_task_intervals(
        da,
        finalization_manager,
        storage_path.path(),
        hooks,
        shutdown_sender,
        Duration::from_millis(20000),
        Duration::from_millis(1000),
    )
    .await
    .unwrap();
    blob_sender
}

#[tokio::test(flavor = "multi_thread")]
async fn blob_sender_posts_data_to_da() -> anyhow::Result<()> {
    let (shutdown_sender, _shutdown_receiver) = watch::channel(());
    let (da, _da_dir) = create_da().await;
    let mut blob_sender = create_blob_sender(da.clone(), shutdown_sender).await;

    let data_1 = {
        let blob_id = 11u8;
        let data = Arc::new([blob_id, 2, 3, 4, 5]);
        blob_sender
            .publish_batch_blob(data.clone(), blob_id as BlobInternalId)
            .await?;
        data
    };

    let data_2 = {
        let blob_id = 12u8;
        let data = Arc::new([blob_id, 2, 3, 4, 5]);
        blob_sender
            .publish_batch_blob(data.clone(), blob_id as BlobInternalId)
            .await?;
        data
    };

    sleep(Duration::from_secs(1)).await;
    let submissions = blob_sender.nb_of_concurrent_blob_submissions();
    assert_eq!(submissions, 2);

    {
        da.produce_block_now().await?;
        sleep(Duration::from_secs(1)).await;
        assert_data_at(&da, data_1.as_slice(), 1).await;
        assert_data_at(&da, data_2.as_slice(), 1).await;

        let submissions = blob_sender.nb_of_concurrent_blob_submissions();
        assert_eq!(submissions, 0);
    }

    Ok(())
}

async fn data_at<Da: DaService>(da: &Da, height: u64) -> Vec<Vec<u8>> {
    let da_block: <Da as DaService>::FilteredBlock = da.get_block_at(height).await.unwrap();
    let batch_blobs = da.extract_relevant_blobs(&da_block).batch_blobs;

    let mut data = Vec::default();

    for mut b in batch_blobs {
        let batch_data = b.full_data().to_vec();
        data.push(batch_data);
    }

    data
}

async fn assert_data_at<Da: DaService>(da: &Da, data: &[u8], height: u64) {
    let batches_from_da = data_at(da, height).await;
    for batch_data in batches_from_da {
        if batch_data.as_slice() == data {
            return;
        }
    }
    panic!("Data missing on DA")
}
