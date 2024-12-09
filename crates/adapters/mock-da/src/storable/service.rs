//! Data Availability service is a controller of [`StorableMockDaLayer`].
use core::time::Duration;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use sov_rollup_interface::common::HexHash;
use sov_rollup_interface::da::{
    BlobReaderTrait, BlockHeaderTrait, DaSpec, RelevantBlobs, RelevantProofs,
};
use sov_rollup_interface::node::da::{DaService, MaybeRetryable, SlotData, SubmitBlobReceipt};
use sov_rollup_interface::node::{future_or_shutdown, FutureOrShutdownOutput};
use tokio::sync::{broadcast, watch, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep};

use crate::storable::layer::StorableMockDaLayer;
use crate::types::WAIT_ATTEMPT_PAUSE;
use crate::{
    MockAddress, MockBlock, MockBlockHeader, MockDaConfig, MockDaSpec, MockDaVerifier, MockFee,
};

const DEFAULT_BLOCK_WAITING_TIME: Duration = Duration::from_secs(120);
// Time to accommodate rare cases of lock waiting time or latency to the database.
const EXTRA_TIME_FOR_MAX_BLOCK: Duration = Duration::from_secs(10);

/// Defines how StorableMockService should produce new blocks.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockProducing {
    /// Produced a new block at every time. Not guaranteed to have precise block time.
    Periodic(Duration),
    /// Produces a new block at every submission of a batch only, not proof.
    /// Means single batch blob per block, but can be batch and proof blobs.
    /// Inner duration is a timeout for a new block to be submitted.
    OnBatchSubmit(Duration),
    /// Produces a new block at every submission of a batch or proof.
    OnAnySubmit(Duration),
    /// Block producing is controlled externally.
    Manual,
}

impl BlockProducing {
    fn get_max_waiting_time_for_block(&self) -> Duration {
        match self {
            BlockProducing::OnBatchSubmit(duration)
            | BlockProducing::Periodic(duration)
            | BlockProducing::OnAnySubmit(duration) => *duration + EXTRA_TIME_FOR_MAX_BLOCK,
            // Use a large number to prevent infinite blocking.
            BlockProducing::Manual => DEFAULT_BLOCK_WAITING_TIME * 1000,
        }
    }

    /// Spawns periodic block producing. Useful for testing or custom setup.
    fn spawn_block_producing_if_needed(
        &self,
        shutdown_receiver: watch::Receiver<()>,
        da_layer: Arc<RwLock<StorableMockDaLayer>>,
    ) -> Option<JoinHandle<()>> {
        let BlockProducing::Periodic(duration) = self else {
            return None;
        };

        let duration = *duration;
        Some(tokio::spawn(async move {
            tracing::debug!(interval = ?duration, "Spawning a task for periodic producing");
            loop {
                match future_or_shutdown(tokio::time::sleep(duration), &shutdown_receiver).await {
                    FutureOrShutdownOutput::Shutdown => {
                        tracing::debug!("Received shutdown signal, stopping block production...");
                        break;
                    }
                    FutureOrShutdownOutput::Output(_) => {
                        let mut da_layer = da_layer.write().await;
                        match da_layer.produce_block().await {
                            Ok(_) => {}
                            Err(err) => {
                                tracing::warn!(error = ?err, "Error producing new block. Will try next time.");
                            }
                        }
                    }
                }
            }
            tracing::info!("Periodic block producing is stopped");
        }))
    }
}

/// DaService that works on top of [`StorableMockDaLayer`].
#[derive(Clone)]
pub struct StorableMockDaService {
    sequencer_da_address: MockAddress,
    da_layer: Arc<RwLock<StorableMockDaLayer>>,
    block_producing: BlockProducing,
    aggregated_proof_sender: broadcast::Sender<()>,
    block_producer_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl StorableMockDaService {
    fn construct(
        sequencer_da_address: MockAddress,
        da_layer: Arc<RwLock<StorableMockDaLayer>>,
        block_producing: BlockProducing,
        block_producer_handle: Option<JoinHandle<()>>,
    ) -> Self {
        let (aggregated_proof_subscription, mut rec) = broadcast::channel(16);
        tokio::spawn(async move { while rec.recv().await.is_ok() {} });
        Self {
            sequencer_da_address,
            da_layer,
            block_producing,
            aggregated_proof_sender: aggregated_proof_subscription,
            block_producer_handle: Arc::new(Mutex::new(block_producer_handle)),
        }
    }

    /// Create a new [` StorableMockDaService `] with the given address.
    pub fn new(
        sequencer_da_address: MockAddress,
        da_layer: Arc<RwLock<StorableMockDaLayer>>,
        block_producing: BlockProducing,
    ) -> Self {
        Self::construct(sequencer_da_address, da_layer, block_producing, None)
    }

    /// Create a new [` StorableMockDaService `] with the given address and [`BlockProducing::Manual`].
    /// Shorter constructor.
    pub fn new_manual_producing(
        sequencer_da_address: MockAddress,
        da_layer: Arc<RwLock<StorableMockDaLayer>>,
    ) -> Self {
        let (drop_notifier, _) = DropNotifier::build();
        let drop_notifier = Arc::new(drop_notifier);
        Self::new(
            sequencer_da_address,
            da_layer,
            BlockProducing::Manual,
            drop_notifier,
        )
    }

    /// Will receive notification one block before the proof is included on the DA.
    pub fn subscribe_proof_posted(&self) -> broadcast::Receiver<()> {
        self.aggregated_proof_sender.subscribe()
    }

    /// Creates new [`StorableMockDaService`] with given address.
    /// Block producing happens on blob submission.
    /// Data is stored only in memory.
    /// It is very similar to [`crate::MockDaService`] parameters.
    pub async fn new_in_memory(sequencer_da_address: MockAddress, blocks_to_finality: u32) -> Self {
        let da_layer = StorableMockDaLayer::new_in_memory(blocks_to_finality)
            .await
            .expect("Failed to initialize StorableMockDaLayer");
        let producing = BlockProducing::OnBatchSubmit(DEFAULT_BLOCK_WAITING_TIME);
        Self::new(
            sequencer_da_address,
            Arc::new(RwLock::new(da_layer)),
            producing,
        )
    }

    /// Creates new in memory [`StorableMockDaService`] from [`MockDaConfig`].
    pub async fn from_config(config: MockDaConfig, shutdown_receiver: watch::Receiver<()>) -> Self {
        let da_layer = StorableMockDaLayer::new_from_connection(
            &config.connection_string,
            config.finalization_blocks,
        )
        .await
        .expect("Failed to initialize StorableMockDaLayer");
        let block_producing = config.block_producing();
        let da_layer = Arc::new(RwLock::new(da_layer));
        let handle =
            block_producing.spawn_block_producing_if_needed(shutdown_receiver, da_layer.clone());
        Self::construct(config.sender_address, da_layer, block_producing, handle)
    }

    async fn wait_for_height(&self, height: u32) -> anyhow::Result<()> {
        let start_wait = Instant::now();
        let max_waiting_time = self.block_producing.get_max_waiting_time_for_block();
        let mut interval = interval(WAIT_ATTEMPT_PAUSE);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                   let current_height = self.da_layer.read().await.next_height;
                   if current_height > height {
                        return Ok(())
                    }
                }
                _ = sleep(max_waiting_time.saturating_sub(start_wait.elapsed())) => {
                    anyhow::bail!("No block at height={height} has been sent in {:?}", max_waiting_time);
                }
            }
        }
    }

    /// Trigger creation of a new block on underlying [`StorableMockDaLayer`].
    pub async fn produce_block_now(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.block_producing == BlockProducing::Manual,
            "Can only trigger block producing in Manual mode, but current is {:?}",
            self.block_producing
        );
        let mut da_layer = self.da_layer.write().await;
        da_layer.produce_block().await
    }
}

#[async_trait]
impl DaService for StorableMockDaService {
    type Spec = MockDaSpec;
    type Config = MockDaConfig;
    type Verifier = MockDaVerifier;
    type FilteredBlock = MockBlock;
    type HeaderStream = BoxStream<'static, Result<MockBlockHeader, Self::Error>>;
    type Error = MaybeRetryable<anyhow::Error>;
    type Fee = MockFee;

    async fn get_block_at(&self, height: u64) -> Result<Self::FilteredBlock, Self::Error> {
        tracing::trace!(%height, "Getting block at");
        if height > u32::MAX as u64 {
            return Err(MaybeRetryable::Permanent(anyhow::anyhow!(
                "Height {} is too big for StorableMockDaService. Max is {}",
                height,
                u32::MAX
            )));
        }

        let height = height as u32;

        self.wait_for_height(height).await?;

        let da_layer = self.da_layer.read().await;

        let block = da_layer
            .get_block_at(height)
            .await
            .map_err(MaybeRetryable::Transient)?;
        tracing::debug!(block_header = %block.header().display(), "Block retrieved");
        Ok(block)
    }

    async fn get_last_finalized_block_header(
        &self,
    ) -> Result<<Self::Spec as DaSpec>::BlockHeader, Self::Error> {
        self.da_layer
            .read()
            .await
            .get_last_finalized_block_header()
            .await
            .map_err(MaybeRetryable::Transient)
    }

    async fn subscribe_finalized_header(&self) -> Result<Self::HeaderStream, Self::Error> {
        let receiver = self
            .da_layer
            .read()
            .await
            .finalized_header_sender
            .subscribe();

        let stream = futures::stream::unfold(receiver, |mut receiver| async move {
            match receiver.recv().await {
                Ok(header) => Some((Ok(header), receiver)),
                Err(_) => None,
            }
        });

        Ok(stream.boxed())
    }

    async fn get_head_block_header(
        &self,
    ) -> Result<<Self::Spec as DaSpec>::BlockHeader, Self::Error> {
        self.da_layer
            .read()
            .await
            .get_head_block_header()
            .await
            .map_err(MaybeRetryable::Transient)
    }

    fn extract_relevant_blobs(
        &self,
        block: &Self::FilteredBlock,
    ) -> RelevantBlobs<<Self::Spec as DaSpec>::BlobTransaction> {
        block.as_relevant_blobs()
    }

    async fn get_extraction_proof(
        &self,
        block: &Self::FilteredBlock,
        _blobs: &RelevantBlobs<<Self::Spec as DaSpec>::BlobTransaction>,
    ) -> RelevantProofs<
        <Self::Spec as DaSpec>::InclusionMultiProof,
        <Self::Spec as DaSpec>::CompletenessProof,
    > {
        block.get_relevant_proofs()
    }

    async fn send_transaction(
        &self,
        blob: &[u8],
        _fee: Self::Fee,
    ) -> Result<SubmitBlobReceipt<<Self::Spec as DaSpec>::TransactionId>, Self::Error> {
        tracing::debug!(batch = hex::encode(blob), "Submitting a batch");
        let blob_hash = {
            let da_layer = self.da_layer.read().await;
            da_layer
                .submit_batch(blob, &self.sequencer_da_address)
                .await?
        };
        match &self.block_producing {
            BlockProducing::OnBatchSubmit(_) | BlockProducing::OnAnySubmit(_) => {
                let mut da_layer = self.da_layer.write().await;
                da_layer.produce_block().await?;
            }
            BlockProducing::Periodic(_) | BlockProducing::Manual => (),
        }
        Ok(SubmitBlobReceipt {
            blob_hash: HexHash::new(blob_hash.0),
            da_transaction_id: blob_hash,
        })
    }

    async fn send_proof(
        &self,
        aggregated_proof_data: &[u8],
        _fee: Self::Fee,
    ) -> Result<SubmitBlobReceipt<<Self::Spec as DaSpec>::TransactionId>, Self::Error> {
        tracing::debug!(
            blob = hex::encode(aggregated_proof_data),
            "Sending an aggregated proof"
        );
        let blob_hash = {
            let da_layer = self.da_layer.read().await;
            da_layer
                .submit_proof(aggregated_proof_data, &self.sequencer_da_address)
                .await?
        };

        self.aggregated_proof_sender
            .send(())
            .map_err(|e| MaybeRetryable::Transient(e.into()))?;

        match &self.block_producing {
            BlockProducing::OnBatchSubmit(_) => {
                tracing::debug!("Proof submission won't produce new DA block");
            }
            BlockProducing::OnAnySubmit(_) => {
                let mut da_layer = self.da_layer.write().await;
                da_layer.produce_block().await?;
            }
            BlockProducing::Periodic(_) | BlockProducing::Manual => (),
        }

        Ok(SubmitBlobReceipt {
            blob_hash: HexHash::new(blob_hash.0),
            da_transaction_id: blob_hash,
        })
    }

    async fn get_proofs_at(&self, height: u64) -> Result<Vec<Vec<u8>>, Self::Error> {
        let blobs = self.get_block_at(height).await?.proof_blobs;
        Ok(blobs
            .into_iter()
            .map(|mut proof_blob| proof_blob.full_data().to_vec())
            .collect())
    }

    async fn estimate_fee(&self, _blob_size: usize) -> Result<Self::Fee, Self::Error> {
        Ok(MockFee::zero())
    }

    fn take_background_join_handle(&self) -> Option<JoinHandle<()>> {
        self.block_producer_handle.lock().unwrap().take()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rand::Rng;

    use super::*;
    use crate::types::GENESIS_HEADER;

    async fn check_consistency(
        da_service: &StorableMockDaService,
        expected_blobs_count: usize,
    ) -> anyhow::Result<()> {
        let mut prev_block_hash = GENESIS_HEADER.prev_hash;

        let head_block = da_service.get_head_block_header().await?;

        let mut total_blobs_count = 0;
        for height in 0..=head_block.height() {
            let block = da_service.get_block_at(height).await?;
            assert_eq!(height, block.header().height());
            assert_eq!(prev_block_hash, block.header().prev_hash());
            prev_block_hash = block.header().hash();

            total_blobs_count += block.batch_blobs.len() + block.proof_blobs.len();
        }

        assert_eq!(
            expected_blobs_count, total_blobs_count,
            "total blobs count do no match"
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multiple_threads_producing_reading() -> anyhow::Result<()> {
        let da_layer = Arc::new(RwLock::new(StorableMockDaLayer::new_in_memory(0).await?));
        let block_time = Duration::from_millis(50);
        let block_producing = BlockProducing::Periodic(block_time);

        let (shutdown_sender, mut shutdown_receiver) = tokio::sync::watch::channel(());
        shutdown_receiver.mark_unchanged();

        let producing_handle =
            block_producing.spawn_block_producing_if_needed(shutdown_receiver, da_layer.clone());

        let services_count = 20;
        let blobs_per_service = 50;

        let mut services_blobs: Vec<Vec<(Duration, Vec<u8>)>> = Vec::with_capacity(services_count);

        let mut rng = rand::thread_rng();
        for i in 0..services_count {
            let mut this_service_blobs = Vec::with_capacity(blobs_per_service);
            for j in 0..blobs_per_service {
                let blob = vec![(i as u8).saturating_mul(j as u8); 8];
                let sleep_time = Duration::from_millis(rng.gen_range(5..=40));
                this_service_blobs.push((sleep_time, blob));
            }
            services_blobs.push(this_service_blobs);
        }

        let mut handlers = Vec::new();
        for (idx, this_service_blobs) in services_blobs.into_iter().enumerate() {
            let this_da_layer = da_layer.clone();
            let this_block_producing = block_producing.clone();
            let address = MockAddress::new([idx as u8; 32]);
            let fee = MockFee::zero();
            handlers.push(tokio::spawn(async move {
                let da_service =
                    StorableMockDaService::new(address, this_da_layer, this_block_producing);
                for (wait, blob) in this_service_blobs {
                    sleep(wait).await;
                    da_service.send_transaction(&blob, fee).await.unwrap();
                }
            }));
        }

        for handler in handlers {
            handler.await?;
        }
        // Sleep extra block time so all blocks are produced.
        sleep(block_time * 2).await;

        let da_service =
            StorableMockDaService::new(MockAddress::new([1; 32]), da_layer, block_producing);
        check_consistency(&da_service, services_count * blobs_per_service).await?;

        shutdown_sender.send(())?;
        drop(da_service);
        producing_handle.unwrap().await?;

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn querying_height_above_u32_max() -> anyhow::Result<()> {
        let producing = BlockProducing::OnBatchSubmit(Duration::from_millis(10));
        let mut service = StorableMockDaService::new_in_memory(MockAddress::new([0; 32]), 0).await;
        service.block_producing = producing;

        let height_1 = u32::MAX as u64;
        let height_2 = u32::MAX as u64 + 1;

        let result_1 = service.get_block_at(height_1).await;
        assert!(result_1.is_err());
        let err = result_1.unwrap_err().to_string();
        assert_eq!("No block at height=4294967295 has been sent in 10.01s", err);

        let result_2 = service.get_block_at(height_2).await;
        assert!(result_2.is_err());
        let err = result_2.unwrap_err().to_string();
        assert_eq!(
            "Height 4294967296 is too big for StorableMockDaService. Max is 4294967295",
            err
        );

        Ok(())
    }
}
