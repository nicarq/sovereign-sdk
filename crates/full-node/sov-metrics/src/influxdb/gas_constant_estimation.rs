use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;

use crate::influxdb::tracker::SovRollupMetric;
use crate::influxdb::Metric;
use crate::{timestamp, MetricsTracker};

thread_local! {
    /// A map of gas constants and their associated weight.
    pub static GAS_CONSTANTS: RefCell<GasConstantTracker> = RefCell::new(GasConstantTracker::default());
}

/// A structure used to track the usage of gas constants.
#[derive(Debug, Clone, Default, derive_more::Deref, derive_more::DerefMut)]
pub struct GasConstantTracker(HashMap<String, i64>);

impl GasConstantTracker {
    /// Returns the difference between the current and the previous gas constant usage.
    /// Consumes both trackers.
    pub fn diff(mut self, previous: Self) -> Self {
        for (constant, weight) in previous.0.into_iter() {
            if let Some(current_weight) = self.0.get(&constant) {
                self.0.insert(constant, *current_weight - weight);
            }
        }

        self
    }

    /// Emits the gas constant usage as telegraf metrics.
    pub fn report_gas_constants_usage(
        &self,
        method_name: &str,
        tagged_inputs: Vec<(String, String)>,
    ) {
        for (constant, weight) in self.0.iter() {
            crate::track_metrics(|tracker| {
                let point = GasConstantMetric {
                    name: method_name.to_string(),
                    constant: constant.to_string(),
                    num_invocations: *weight,
                    metadata: tagged_inputs.clone(),
                };
                tracker.track_gas_constants_usage(point);
            });
        }
    }
}

#[derive(Debug)]
pub struct GasConstantMetric {
    /// Name of the caller site, usually a function or method
    pub name: String,
    /// The gas constant tracked
    pub constant: String,
    /// A numerical value representing the number of invocations of the gas constant
    pub num_invocations: i64,
    /// Additional metadata to be included in the metrics. The metadata is added as a
    /// measurement attribute according to the [influxdb line protocol](https://docs.influxdata.com/influxdb/cloud/reference/syntax/line-protocol/)
    /// We are parsing the metadata in the `tag_key=tag_value` format of influxdb.
    /// This can be used to filter metrics data in telegraf, by querying metrics for some
    /// specific metadata.
    pub metadata: Vec<(String, String)>,
}

impl MetricsTracker {
    /// Tracks HTTP-related metrics.
    fn track_gas_constants_usage(&self, point: GasConstantMetric) {
        let timestamp = timestamp();

        self.submit(SovRollupMetric::GasConstantUsage(timestamp, point));
    }
}

impl Metric for GasConstantMetric {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        // We are adding the metadata as measurmement tags in the influxdb line protocol.
        let mut parsed_metadata = String::new();

        if !self.metadata.is_empty() {
            parsed_metadata = format!(
                "{},",
                self.metadata
                    .iter()
                    .map(|(key, value)| format!("{}={}", key, value))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        };

        write!(
            buffer,
            "sov_rollup_gas_constant,name={},{}constant={} num_invocations={}",
            self.name, parsed_metadata, self.constant, self.num_invocations
        )?;
        Ok(())
    }
}
