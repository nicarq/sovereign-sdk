use std::fmt::Display;

use crate::ResponseOutput;

/// Contains details about a single HTTP request measurement.
#[derive(Debug)]
pub struct Measurement {
    /// The time it took to complete the request in nanoseconds.
    pub time: u128,
    /// Information about the response.
    pub output: anyhow::Result<ResponseOutput>,
}

/// The Report is a collection of measurements.
#[derive(Debug)]
pub struct Report {
    /// The measurements for a given report
    pub measurements: Vec<Measurement>,
}

impl Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Report {{ measurements: {:?} }}", self.measurements)?;
        Ok(())
    }
}
