//! Logging utilities and defaults.

use std::env;
use std::str::FromStr;

use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

/// Default [`tracing`] initialization for the rollup node.
pub fn initialize_logging() {
    let env_filter = env::var("RUST_LOG").unwrap_or_else(|_| default_rust_log_value().to_string());

    let subscriber = tracing_subscriber::registry()
        .with(fmt::layer().with_filter(EnvFilter::from_str(&env_filter).unwrap()));

    if cfg!(tokio_unstable) {
        subscriber
            .with(
                // See <https://github.com/tokio-rs/console?tab=readme-ov-file#using-it>.
                console_subscriber::spawn()
                    .with_filter(EnvFilter::from_str("tokio=trace,runtime=trace").unwrap()),
            )
            .init();
    } else {
        subscriber.init();
    }

    log_info_about_logging(&env_filter);
    set_tracing_panic_hook();
}

/// A good default for [`EnvFilter`] when `RUST_LOG` is not set.
pub fn default_rust_log_value() -> String {
    [
        "debug", // Default logging level.
        "hyper=info",
        "risc0_zkvm=warn",
        "jmt=info",
        "jsonrpsee-server=info",
        "jsonrpsee-client=info",
        "reqwest=info",
        "sqlx=warn",
        "tiny_http=warn",
        "tower_http=info",
        "tungstenite=info",
        "risc0_circuit_rv32im=info",
        "risc0_zkp::verify=info",
    ]
    .join(",")
}

// No need to make this public, it's an implementation detail of
// [`initialize_logging`].
fn log_info_about_logging(current_env_filter: &str) {
    // Most users won't know about `RUST_LOG`, so let's remind them. Let's
    // also print the current filter so they can copy-paste it and tweak it.
    info!(
        RUST_LOG = current_env_filter,
        "Logging initialized; you can restart the node with a custom `RUST_LOG` env. var. to customize logging filtering"
    );

    let tokio_console_info_url = "https://github.com/tokio-rs/console";
    if cfg!(tokio_unstable) {
        info!(
            tokio_console_info_url,
            "The Tokio debugging console is available",
        );
    } else {
        info!(
            tokio_console_info_url,
            "The Tokio debugging console will not be available; must compile with `cfg(tokio_unstable)` to enable it",
        );
    }
}

/// Adds [`tracing_panic::panic_hook`] to the panic hook.
pub fn set_tracing_panic_hook() {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        tracing_panic::panic_hook(panic_info);
        prev_hook(panic_info);
    }));
}
