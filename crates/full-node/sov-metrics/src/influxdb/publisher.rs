//! Tasks responsible for actual metrics submission.

use crate::influxdb::config::MonitoringConfig;

pub(crate) async fn metrics_publisher_task(
    mut shutdown_receiver: tokio::sync::watch::Receiver<()>,
    mut receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
    config: &MonitoringConfig,
) {
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

    loop {
        let measurement = tokio::select! {
            _ = shutdown_receiver.changed() => {
                break;
            }
            res = receiver.recv() => {
                match res {
                    None => {
                        tracing::info!("Metrics channel has been closed before shutdown signal received, shutting down");
                        break;
                    }
                    Some(m) => m
                }
            }
        };

        // Short-circuit in case if metric exceed buffer size and buffer is empty.
        // If buffer is not empty, go standard route, first submitting what is there and only then going to next iteration.
        if measurement.len() > max_buffer_size && buffer.is_empty() {
            tracing::warn!("Received measurement exceeds max buffer size, submitting immediately");
            send_metrics(&socket, &measurement, config.telegraf_address).await;
            continue;
        }

        // One for '\n'
        let next_size = buffer.len() + measurement.len() + 1;
        // Exceed max size, need to submit the packet first.
        if next_size > max_buffer_size {
            send_metrics(&socket, &buffer, config.telegraf_address).await;
            // Clearing even in case of error, otherwise the UDP packet can grow too much.
            buffer.clear();
        }
        if !buffer.is_empty() {
            buffer.push(b'\n');
        }
        buffer.extend(measurement);
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
    match tokio::time::timeout(TIMEOUT, receiver.recv()).await {
        Ok(s) => s,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test relies on known metric size to check that the task aggregates data,
    /// but submits it when a threshold is crossed.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_publisher_accumulates_and_submits() -> anyhow::Result<()> {
        let sample_metric: &[u8; 23] = b"sov-test-metric value=1";
        let first_chunk = 2;
        let second_chunk = 3;
        let max_udp_size = sample_metric.len() * (first_chunk + second_chunk);
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

        let (shutdown_sender, mut shutdown_receiver) = tokio::sync::watch::channel(());
        shutdown_receiver.mark_unchanged();
        let (sender, receiver) = tokio::sync::mpsc::channel(10);
        let task_handle = tokio::spawn(async move {
            metrics_publisher_task(shutdown_receiver, receiver, &monitoring_config).await;
        });

        for _ in 0..first_chunk {
            sender.send(sample_metric.to_vec()).await?;
        }

        assert!(receive_with_timeout(&mut metrics_back_receiver)
            .await
            .is_none());

        for _ in 0..second_chunk {
            sender.send(sample_metric.to_vec()).await?;
        }

        let metric_string = std::str::from_utf8(&sample_metric[..])?;

        for _ in 0..(total_send - 1) {
            let metric = receive_with_timeout(&mut metrics_back_receiver)
                .await
                .unwrap();
            assert_eq!(metric, metric_string);
        }

        // Nothing is left in channel.
        assert!(receive_with_timeout(&mut metrics_back_receiver)
            .await
            .is_none());

        // Correct shutdown
        let _ = shutdown_sender.send(());
        task_handle.await?;
        Ok(())
    }

    /// This test relies on known metric size to check that the task aggregates data.
    /// It demonstrates, that max packet size option is recommended and not strictly followed.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_metrics_exceed_defined_packed_size() -> anyhow::Result<()> {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let sample_metric = b"sov-test-metric value=1";
        let monitoring_config = MonitoringConfig {
            telegraf_address: socket.local_addr()?,
            max_datagram_size: Some(1),
            max_pending_metrics: None,
        };

        let (metrics_back_sender, mut metrics_back_receiver) = tokio::sync::mpsc::channel(1);
        spawn_metrics_udp_receiver(socket, metrics_back_sender.clone());

        let (shutdown_sender, mut shutdown_receiver) = tokio::sync::watch::channel(());
        shutdown_receiver.mark_unchanged();
        let (sender, receiver) = tokio::sync::mpsc::channel(10);
        let task_handle = tokio::spawn(async move {
            metrics_publisher_task(shutdown_receiver, receiver, &monitoring_config).await;
        });

        sender.send(sample_metric.to_vec()).await?;

        assert!(receive_with_timeout(&mut metrics_back_receiver)
            .await
            .is_some());

        let _ = shutdown_sender.send(());
        task_handle.await?;
        Ok(())
    }
}
