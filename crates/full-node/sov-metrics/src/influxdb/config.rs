/// Configuration of Sovereign monitoring
#[derive(
    Debug, Clone, serde::Deserialize, serde::Serialize, derivative::Derivative, schemars::JsonSchema,
)]
#[derivative(PartialEq)]
pub struct MonitoringConfig {
    /// UDP socket where Telegraf service is active, something like 127.0.0.1:8094.
    /// Localhost is preferred for latency and reliability reasons.
    pub telegraf_address: std::net::SocketAddr,
    /// Defines how many measurements a rollup node will accumulate before sending it to the Telegraf.
    /// It is expected from the rollup node to produce metrics all the time,
    /// so measurements are buffered by size and not sent by time.
    /// and below 67 KB, which is the maximal UDP packet size.
    /// It also means that if a single serialized metric is larger than this value, a UDP packet will be larger.
    ///
    /// Note: to disable buffering, set this value to `Some(1)`.
    pub max_datagram_size: Option<u32>,
    /// How many metrics are allowed to be in pending state, before new metrics will be dropped.
    /// This is a number of metrics, not serialized bytes.
    /// The total number of bytes to be held in memory might vary per metric + `max_datagram_size`
    pub max_pending_metrics: Option<u32>,
}

impl MonitoringConfig {
    /// Standard telegraf port on localhost and default parameters.
    pub const fn standard() -> Self {
        Self::default_on_port(8094)
    }

    /// On a specified localhost port with default parameters.
    pub const fn default_on_port(port: u16) -> Self {
        Self {
            telegraf_address: std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::LOCALHOST,
                port,
            )),
            max_datagram_size: None,
            max_pending_metrics: None,
        }
    }

    pub(crate) fn get_max_datagram_size(&self) -> u32 {
        // https://stackoverflow.com/a/35697810/995270
        // Safe max UDP size.
        // A little bit conservative.
        self.max_datagram_size.unwrap_or(508)
    }

    pub(crate) fn get_max_pending_metrics(&self) -> u32 {
        self.max_pending_metrics.unwrap_or(1000)
    }
}
