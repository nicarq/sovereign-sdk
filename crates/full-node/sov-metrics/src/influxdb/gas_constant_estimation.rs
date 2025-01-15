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
#[derive(Clone, Default, derive_more::Deref, derive_more::DerefMut)]
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
    pub fn report_gas_constants_usage(&self, method_name: &str) {
        for (constant, weight) in self.0.iter() {
            crate::track_metrics(|tracker| {
                let point = GasConstantMetric {
                    name: method_name.to_string(),
                    constant: constant.to_string(),
                    num_invocations: *weight,
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
        write!(
            buffer,
            "sov_rollup_gas_constant,name={},constant={} num_invocations={}",
            self.name, self.constant, self.num_invocations
        )?;
        Ok(())
    }
}
