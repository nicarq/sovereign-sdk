//! InfluxDB metrics for Sovereign rollups.

#[cfg(feature = "native")]
mod config;
#[cfg(feature = "native")]
mod publisher;
#[cfg(feature = "native")]
mod tracker;

#[cfg(feature = "native")]
pub use config::MonitoringConfig;
#[cfg(feature = "native")]
pub use tracker::{init_metrics_tracker, RunnerMetrics};

/// Struct for tracking Sovereign metrics.
///
/// Hides underlying monitoring system implementation.
#[cfg(feature = "native")]
#[derive(Clone)]
pub struct MetricsTracker {
    sender: tokio::sync::mpsc::Sender<Vec<u8>>,
}

/// Stub struct for non-native code
#[cfg(not(feature = "native"))]
#[derive(Clone)]
pub struct MetricsTracker {}

/// Stub function for non-native code
#[cfg(not(feature = "native"))]
pub fn track_metrics<F>(_f: F)
where
    F: FnOnce(&MetricsTracker),
{
}

/// Applies a function to the global [`MetricsTracker`] instance if and only if the
/// `native` feature is enabled and [`MetricsTracker`] has been initialized.
#[cfg(feature = "native")]
pub fn track_metrics<F>(f: F)
where
    F: FnOnce(&MetricsTracker),
{
    match std::sync::OnceLock::get(&tracker::METRICS_TRACKER) {
        None => {
            tracing::warn!("Submitting metrics to uninitialized metrics tracker. Submitted metrics will be dropped. Please call `sov_metrics::init_metrics_tracker` to prevent data loss.");
        }
        Some(m) => {
            f(m);
        }
    };
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::influxdb::publisher::{
        metrics_publisher_task, receive_with_timeout, spawn_metrics_udp_receiver,
    };
    use crate::influxdb::tracker::timestamp;

    /// Starts publisher tasks and checks that tracker pushes all required metrics
    #[tokio::test(flavor = "multi_thread")]
    async fn test_runner_metrics_published() -> anyhow::Result<()> {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let monitoring_config = MonitoringConfig {
            telegraf_address: socket.local_addr()?,
            max_datagram_size: None,
            max_pending_metrics: None,
        };

        let (metrics_back_sender, mut metrics_back_receiver) = tokio::sync::mpsc::channel(100);
        spawn_metrics_udp_receiver(socket, metrics_back_sender.clone());

        let (shutdown_sender, mut shutdown_receiver) = tokio::sync::watch::channel(());
        shutdown_receiver.mark_unchanged();
        let (sender, receiver) = tokio::sync::mpsc::channel(10);
        let task_handle = tokio::spawn(async move {
            metrics_publisher_task(shutdown_receiver, receiver, &monitoring_config).await;
        });

        let tracker = MetricsTracker { sender };

        let start = timestamp();
        tracker.track_runner_metrics(RunnerMetrics {
            da_height_processed: 12333,
            sync_distance: 55768,
            get_block_time: std::time::Duration::from_millis(1000),
            batches_processed: 2084,
            batch_bytes_processed: 785,
            transactions_processed: 4444,
            proofs_processed: 123854,
            proof_bytes_processed: 5432341,
            process_slot_time: std::time::Duration::from_millis(1001),
            apply_slot_time: std::time::Duration::from_millis(1002),
            stf_transition_time: std::time::Duration::from_millis(1003),
            extract_blobs_time: std::time::Duration::from_millis(1004),
            extraction_proof_time: std::time::Duration::from_millis(1005),
        });
        let finish = timestamp();

        // TODO: Verify exact metrics values;
        let total_expected_number_of_metrics = 14;
        let mut received_number_of_metrics = 0;
        loop {
            let metric = match receive_with_timeout(&mut metrics_back_receiver).await {
                None => break,
                Some(m) => m,
            };
            received_number_of_metrics += 1;
            let timestamp = u128::from_str(
                metric
                    .split(' ')
                    .last()
                    .expect("Timestamp not found for metric"),
            )
            .expect("Failed to parse timestamp");
            assert!(
                timestamp >= start,
                "Incorrect timestamp from metric: lagging behind"
            );
            assert!(
                timestamp <= finish,
                "Incorrect timestamp from metric: running upfront"
            );
        }

        // At least 3 metrics should be squeezed, otherwise buffering does not seem to worth it.
        assert!(received_number_of_metrics > 2);
        assert!(received_number_of_metrics <= total_expected_number_of_metrics);

        let _ = shutdown_sender.send(());
        task_handle.await?;
        Ok(())
    }
}
