//! Utilities for parallel fetching of the finalized blocks.

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::FuturesOrdered;
use futures::{Stream, StreamExt};
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::node::da::{DaService, SlotData};
use tokio::sync::mpsc::Receiver;

// With the block size up to 10 MB, it should fit into 32 GB of RAM.
const MAX_BLOCKS: usize = 1_000;

/// Service that pre-fetcher blocks from given start height up to last finalized height at the moment of construction.
/// After that it proxies all requests to underlying DaService.
pub struct FinalizedBlocksBulkFetcher<Da: DaService> {
    da_service: Arc<Da>,
    blocks: Receiver<Da::FilteredBlock>,
    start_height: u64,
    last_finalized_height: u64,
}

impl<Da> FinalizedBlocksBulkFetcher<Da>
where
    Da: DaService,
{
    pub async fn new(
        da_service: Arc<Da>,
        start_height: u64,
        bulk_size: u8,
    ) -> anyhow::Result<Self> {
        let (blocks_sender, blocks_receiver) = tokio::sync::mpsc::channel(MAX_BLOCKS);

        let last_finalized_height = da_service
            .get_last_finalized_block_header()
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .height();

        let block_fetcher = BlockFetcher::new(
            da_service.clone(),
            blocks_sender,
            start_height,
            last_finalized_height,
            bulk_size,
        );

        let _handle = tokio::spawn(async {
            match block_fetcher.run().await {
                Ok(()) => {
                    tracing::debug!("BlockFetcher task has completed");
                }
                Err(e) => {
                    tracing::error!(error = ?e, "BlockFetcher task has failed");
                }
            };
        });

        Ok(Self {
            da_service,
            blocks: blocks_receiver,
            start_height,
            last_finalized_height,
        })
    }

    /// Wrapper around [`DaService::get_block_at`]
    pub async fn get_block_at(&mut self, height: u64) -> Result<Da::FilteredBlock, Da::Error> {
        if height > self.last_finalized_height || height < self.start_height {
            tracing::trace!(
                height,
                start_height = self.start_height,
                last_finalized_height = self.last_finalized_height,
                "Requested height is outside of pre-fetched range, querying DaService directly"
            );
            return self.da_service.get_block_at(height).await;
        }

        while let Some(block) = self.blocks.recv().await {
            let block_height = block.header().height();
            self.start_height = block_height;
            if block_height == height {
                return Ok(block);
            }
            tracing::warn!(
                block_header = %block.header().display(),
                "Skipping pre-fetched block from the channel. Reading out of order might've been occurred");
        }

        tracing::warn!(
            height,
            "Didn't find block in pre-fetched when it should've been, calling DaService"
        );
        self.da_service.get_block_at(height).await
    }
}

struct BlockFetcher<Da: DaService> {
    da_service: Arc<Da>,
    blocks: tokio::sync::mpsc::Sender<Da::FilteredBlock>,
    start_height: u64,
    last_finalized_height: u64,
    // Defines how many requests can be made concurrently to a target DaService
    bulk_size: u8,
}

impl<Da> BlockFetcher<Da>
where
    Da: DaService,
{
    fn new(
        da_service: Arc<Da>,
        blocks: tokio::sync::mpsc::Sender<Da::FilteredBlock>,
        start_height: u64,
        last_finalized_height: u64,
        bulk_size: u8,
    ) -> Self {
        BlockFetcher {
            da_service,
            blocks,
            start_height,
            last_finalized_height,
            bulk_size,
        }
    }

    async fn fetch_blocks_in_range(
        &self,
        start: u64,
        end: u64,
    ) -> Pin<Box<dyn Stream<Item = anyhow::Result<Da::FilteredBlock>> + Send>> {
        let futures: FuturesOrdered<_> = (start..=end)
            .map(|height| {
                let da_service = Arc::clone(&self.da_service);
                async move {
                    da_service
                        .get_block_at(height)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))
                }
            })
            .collect();

        Box::pin(futures)
    }

    async fn run(mut self) -> anyhow::Result<()> {
        tracing::trace!(
            start = self.start_height,
            last_finalized_height = self.last_finalized_height,
            "Running bulk block fetcher"
        );
        while self.start_height < self.last_finalized_height {
            let start_height = self.start_height;
            let end_height = std::cmp::min(
                start_height + self.bulk_size as u64,
                self.last_finalized_height,
            );
            let start = std::time::Instant::now();

            let mut block_stream = self.fetch_blocks_in_range(start_height, end_height).await;
            let mut blocks_fetched = 0;

            while let Some(block_result) = block_stream.next().await {
                let block = block_result?;
                blocks_fetched += 1;
                self.blocks.send(block).await?;
            }

            if blocks_fetched == 0 {
                break;
            }

            tracing::trace!(
                start_height,
                end_height,
                time = ?start.elapsed(),
                blocks = blocks_fetched,
                "Fetched blocks"
            );

            self.start_height = end_height + 1;
        }

        tracing::info!("BlockFetcher synced all finalized headers");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use sov_mock_da::storable::service::StorableMockDaService;
    use sov_mock_da::MockFee;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, EnvFilter};

    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn check_that_all_blocks_are_collected_instant_finality() {
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(EnvFilter::from_str("debug,sqlx=warn,sov_stf_runner=trace").unwrap())
            .init();
        let da_service = StorableMockDaService::new_in_memory(Default::default(), 0).await;
        let blocks_number = 200;
        for i in 1..=blocks_number {
            da_service
                .send_transaction(&[i; 32], MockFee::zero())
                .await
                .unwrap();
        }

        let mut fetcher = FinalizedBlocksBulkFetcher::new(Arc::new(da_service), 0, 10)
            .await
            .unwrap();

        for i in 0..blocks_number {
            let block = fetcher.get_block_at(i as u64).await.unwrap();
            assert_eq!(i as u64, block.header().height());
        }
    }
}
