use clap::Parser;
use sov_modules_api::prelude::tracing;
use sov_soak_testing::setup_rollup;
use tokio::signal::unix::SignalKind;

#[derive(Parser)]
struct Args {
    #[arg(short, long, default_value_t = 12346)]
    /// The port that the axum API server will listen on.
    /// Defaults to 12346.
    axum_port: u16,

    #[arg(short, long, default_value = "soak_data/")]
    storage_path: String,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let _guard = sov_modules_rollup_blueprint::logging::initialize_logging();
    let args = Args::parse();
    let (rollup, _) = setup_rollup(args.storage_path.into(), args.axum_port).await;

    let mut terminate = tokio::signal::unix::signal(SignalKind::terminate())
        .expect("Failed to set up SIGTERM handler");
    let mut quit =
        tokio::signal::unix::signal(SignalKind::quit()).expect("Failed to set up SIGQUIT handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("Received Ctrl+C"),
        _ = terminate.recv() => tracing::info!("Received SIGTERM"),
        _ = quit.recv() => tracing::info!("Received SIGQUIT"),
    }

    rollup.shutdown().await?;

    Ok(())
}
