//! Tasks responsible for actual metrics submission.

use crate::influxdb::config::MonitoringConfig;
use crate::influxdb::SerializableMetric;

pub(crate) async fn metrics_publisher_task(
    mut receiver: tokio::sync::mpsc::Receiver<SerializableMetric>,
    config: &MonitoringConfig,
) {
    tracing::trace!(?config, "Starting metrics publisher task");
    let max_buffer_size = config.get_max_datagram_size() as usize;
    assert!(max_buffer_size > 0, "Max buffer size cannot be zero");
    // Number is based on [`std::net::UdpSocket::send_to`] documentation.
    // If this number is changed, please consult documentation and add handling for partial writes!
    assert!(
        max_buffer_size < 65507,
        "Max buffer size should be less than maximum allowed UDP packet, but it is {}",
        max_buffer_size
    );
    // Binding at any port, it does not matter.
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();

    tracing::trace!(?socket, "Starting metrics publishing task");
    let mut buffer: Vec<u8> = Vec::with_capacity(max_buffer_size);

    while let Some(measurement) = receiver.recv().await {
        tracing::trace!(?measurement, "Received measurement");
        if !buffer.is_empty() {
            buffer.push(b'\n');
        }
        if let Err(error) = measurement.serialize_for_telegraf(&mut buffer) {
            tracing::warn!(?error, "Failed to format measurement, skipping");
        };
        // We know that telegraf format is string based, so for debugging we can print strings:
        tracing::trace!(buffer = ?String::from_utf8_lossy(&buffer), "Serialized measurement into buffer");

        // Exceed max size, need to submit the packet first.
        if buffer.len() > max_buffer_size {
            send_metrics(&socket, &buffer, config.telegraf_address).await;
            // Clearing even in case of error, otherwise the UDP packet can grow too much.
            buffer.clear();
        }
    }

    tracing::debug!("Metrics publishing task has been completed");
}

async fn send_metrics(
    socket: &tokio::net::UdpSocket,
    buffer: &[u8],
    address: std::net::SocketAddr,
) {
    match socket.send_to(buffer, address).await {
        Ok(bytes_written) => {
            if bytes_written < buffer.len() {
                // This means partial writing happened,
                // and according to documentation,
                // it should not be the case for buffer below i32::MAX.
                //
                // Has max buffer size changed?
                tracing::error!(
                    bytes_written,
                    buffer_size = buffer.len(),
                    "UDP Socket wrote less bytes than was passed. This is a bug."
                );
            }
            tracing::trace!(?socket, "Metrics have been successfully send to socket");
        }
        Err(err) => {
            tracing::warn!(error = ?err, "Error publishing metrics");
        }
    }
}

/// Listens on UDP port and converts all received metrics to strings and sends back to a channel.
#[cfg(test)]
pub(crate) fn spawn_metrics_udp_receiver(
    socket: tokio::net::UdpSocket,
    metrics_write: tokio::sync::mpsc::Sender<String>,
) {
    tokio::task::spawn(async move {
        loop {
            let mut buf = [0; 1024];

            match socket.recv_from(&mut buf).await {
                Ok((size, _src)) => {
                    let received = &buf[..size];
                    let measurements = std::str::from_utf8(received).unwrap().split('\n');
                    for m in measurements {
                        metrics_write.send(m.to_owned()).await.unwrap();
                    }
                }
                Err(e) => panic!("Error receiving: {}", e),
            }
        }
    });
}

#[cfg(test)]
pub(crate) async fn receive_with_timeout(
    receiver: &mut tokio::sync::mpsc::Receiver<String>,
) -> Option<String> {
    static TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);
    tokio::time::timeout(TIMEOUT, receiver.recv())
        .await
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::influxdb::Metric;

    #[derive(Clone, Debug)]
    struct SampleMetric(Vec<u8>);

    impl Metric for SampleMetric {
        fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
            buffer.extend_from_slice(&self.0);
            Ok(())
        }
    }

    /// This test relies on known metric size to check that the task aggregates data,
    /// but submits it when a threshold is crossed.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_publisher_accumulates_and_submits() -> anyhow::Result<()> {
        let sample_metric = SampleMetric(b"sov-test-metric value=1".to_vec());
        let first_chunk = 2;
        let second_chunk = 3;
        let max_udp_size = sample_metric.0.len() * (first_chunk + second_chunk);
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;

        let total_send = first_chunk + second_chunk;

        let monitoring_config = MonitoringConfig {
            telegraf_address: socket.local_addr()?,
            max_datagram_size: Some(max_udp_size as u32),
            // Does not matter, we set our own channel size.
            max_pending_metrics: None,
        };

        let (metrics_back_sender, mut metrics_back_receiver) =
            tokio::sync::mpsc::channel(total_send);
        spawn_metrics_udp_receiver(socket, metrics_back_sender.clone());

        let (sender, receiver) = tokio::sync::mpsc::channel(10);
        let _task_handle = tokio::spawn(async move {
            metrics_publisher_task(receiver, &monitoring_config).await;
        });

        for _ in 0..first_chunk {
            let x = Box::new(sample_metric.clone());
            sender.send(x).await?;
        }

        assert!(receive_with_timeout(&mut metrics_back_receiver)
            .await
            .is_none());

        for _ in 0..second_chunk {
            sender.send(Box::new(sample_metric.clone())).await?;
        }

        let metric_string = std::str::from_utf8(&sample_metric.0[..])?;

        for _ in 0..total_send {
            let metric = receive_with_timeout(&mut metrics_back_receiver)
                .await
                .unwrap();
            assert_eq!(metric, metric_string);
        }

        // Nothing is left in the channel.
        assert!(receive_with_timeout(&mut metrics_back_receiver)
            .await
            .is_none());

        Ok(())
    }

    /// This test relies on known metric size to check that the task aggregates data.
    /// It demonstrates that maximum packet size configuration is recommended and not strictly followed.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_metrics_exceed_defined_packed_size() -> anyhow::Result<()> {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let sample_metric = SampleMetric(b"sov-test-metric value=1".to_vec());
        let monitoring_config = MonitoringConfig {
            telegraf_address: socket.local_addr()?,
            max_datagram_size: Some(1),
            max_pending_metrics: None,
        };

        let (metrics_back_sender, mut metrics_back_receiver) = tokio::sync::mpsc::channel(1);
        spawn_metrics_udp_receiver(socket, metrics_back_sender.clone());

        let (sender, receiver) = tokio::sync::mpsc::channel(10);
        let _task_handle = tokio::spawn(async move {
            metrics_publisher_task(receiver, &monitoring_config).await;
        });

        sender.send(Box::new(sample_metric)).await?;

        assert!(receive_with_timeout(&mut metrics_back_receiver)
            .await
            .is_some());

        Ok(())
    }
}
