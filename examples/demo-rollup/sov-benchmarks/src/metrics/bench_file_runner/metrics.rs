//! This module can query metrics from telegraph and store them.

use std::io::{BufWriter, Write};

use futures::{pin_mut, StreamExt};
use reqwest::Client;
use tracing::trace;

use super::ParsedMetricsParameters;

/// Main function that queries metrics from telegraph and stores them to the supplied file.
pub async fn get_metrics(
    start_timestamp: u128,
    end_timestamp: u128,
    metrics_params: ParsedMetricsParameters,
) -> anyhow::Result<()> {
    // Query metrics from telegraph and store them to the output file.
    println!("Querying metrics from InfluxDB...");

    let metrics_query = format!(
        "from(bucket: \"sov-rollup\")
    |> range(start: time(v: {start_timestamp}), stop: time(v: {end_timestamp}))
    {}",
        metrics_params.query_filter
    );

    let post_addr = format!(
        "http://{}/api/v2/query?orgID={}",
        metrics_params.influx_address, metrics_params.influx_org_id
    );

    let client = Client::new();

    let mut response_builder = client
        .post(post_addr)
        .bearer_auth(metrics_params.influx_auth_token)
        .header("Accept", "application/csv")
        .header("Content-type", "application/vnd.flux")
        .body(metrics_query);

    // If the metrics are encoded, then we need to add the Accept-Encoding header.
    if metrics_params.encoded {
        response_builder = response_builder.header("Accept-Encoding", "gzip");
    }

    let response = response_builder.send().await?.error_for_status()?;

    trace!(
        thread = "metrics_storage",
        "Successfully queried metrics from InfluxDB. Writing to file..."
    );

    // Stream the response to the output file.
    let response_stream = response.bytes_stream();
    let mut writer = BufWriter::new(metrics_params.output_file);

    pin_mut!(response_stream);
    while let Some(chunk) = response_stream.next().await {
        match chunk {
            Ok(chunk) => {
                writer.write_all(&chunk)?;
            }
            Err(e) => {
                // This should never happen.
                // If it does, it means that the response is malformed.
                // We should panic, because we can't recover from this.
                // The response is malformed, so we can't even try to recover.
                panic!("Failed to read response chunk: {:?}", e);
            }
        }
    }

    writer.flush()?;

    trace!(
        thread = "metrics_storage",
        "Successfully written metrics to file."
    );

    Ok(())
}
