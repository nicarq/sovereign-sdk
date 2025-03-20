use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use rand::Rng;
use sov_api_spec::Client;
use sov_bank::Bank;
use sov_bank::CallMessageDiscriminants::Transfer;
use sov_modules_api::prelude::arbitrary::Unstructured;
use sov_modules_api::prelude::tracing;
use sov_modules_api::{CryptoSpec, EncodeCall, Runtime, Spec};
use sov_soak_testing::{
    plain_tx_with_default_details, setup_harness, CelestiaRollupSpec, DemoCelestiaRT, DemoMockRT,
    MockDemoRollupSpec, TestGenerator, TestRT,
};
use sov_test_utils::{TestSpec, TransactionType};
use sov_transaction_generator::generators::bank::harness_interface::BankHarness;
use sov_transaction_generator::generators::bank::BankMessageGenerator;
use sov_transaction_generator::generators::basic::BasicModuleRef;
use sov_transaction_generator::interface::rng_utils::get_random_bytes;
use sov_transaction_generator::{Distribution, MessageValidity, Percent};
use tokio::signal::unix::SignalKind;
use tokio::sync::watch::Receiver;
use tokio::task::JoinSet;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum SelectedRuntime {
    /// Generated test runtime, running by the sov-soak-testing
    Test,
    /// demo-stf with Celestia DA
    DemoCelestia,
    /// demo-stf with Mock DA
    DemoMock,
}

#[derive(Parser)]
struct Args {
    #[arg(short, long, default_value = "http://localhost:12346")]
    /// The URL of the rollup node to connect to. Defaults to http://localhost:12346.
    api_url: String,

    #[arg(short, long, default_value = "5")]
    /// The number of workers to spawn - this controls the number of concurrent transactions. Defaults to 5.
    num_workers: u32,

    #[arg(short, long, default_value = "Runtime::Test")]
    runtime: SelectedRuntime,

    #[arg(short, long, default_value = "0")]
    /// The salt to use for RNG. Use this value if you're restarting the generator and want to ensure that the generated
    /// transactions don't overlap with the previous run.
    salt: u32,
}

async fn worker_task(
    client: sov_api_spec::Client,
    rx: Receiver<bool>,
    worker_id: u128,
    num_workers: u32,
    runtime: SelectedRuntime,
) -> anyhow::Result<()> {
    let result = match runtime {
        SelectedRuntime::Test => {
            worker_task_inner::<TestRT, TestSpec>(client, rx, worker_id, num_workers).await
        }
        SelectedRuntime::DemoCelestia => {
            worker_task_inner::<DemoCelestiaRT, CelestiaRollupSpec>(
                client,
                rx,
                worker_id,
                num_workers,
            )
            .await
        }
        SelectedRuntime::DemoMock => {
            worker_task_inner::<DemoMockRT, MockDemoRollupSpec>(client, rx, worker_id, num_workers)
                .await
        }
    };

    if let Err(e) = result {
        tracing::error!("Worker task {worker_id} failed: {}", e);
        std::process::exit(1);
    }
    Ok(())
}

async fn worker_task_inner<R: Runtime<S> + EncodeCall<Bank<S>> + Clone, S: Spec>(
    client: sov_api_spec::Client,
    rx: Receiver<bool>,
    worker_id: u128,
    num_workers: u32,
) -> anyhow::Result<()> {
    let mut nonces: HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64> =
        Default::default();

    let random_bytes = get_random_bytes(100_000_000, worker_id);
    let u = &mut Unstructured::new(&random_bytes[..]);
    let bank_harness = BankHarness::new(BankMessageGenerator::<S>::new(
        Distribution::with_equiprobable_values(vec![Transfer]),
        Percent::one_hundred(),
    ));
    let modules: Vec<BasicModuleRef<S, R>> = vec![Arc::new(bank_harness.clone())];
    let modules = Distribution::with_equiprobable_values(modules);
    let mut generator: TestGenerator<R, S> = setup_harness(worker_id);

    let worker_start = std::time::Instant::now();
    let mut total_txns = 0;
    while !*rx.borrow() {
        let txn_count = {
            // rng must fall out of scope before awaiting anything so this fn is Send
            let mut rng = rand::thread_rng();

            // Do this at the start so we add some jitter to initial API requests
            let sleep_ms = rng.gen_range(25..100);
            std::thread::sleep(Duration::from_millis(sleep_ms));

            rng.gen_range(10..100)
        };

        let mut txns = vec![];
        for _ in 0..txn_count {
            let validity = Distribution::with_equiprobable_values(vec![MessageValidity::Valid]);
            let validity = validity.select_value(u).unwrap();
            let msg = generator.generate(&modules, *validity);
            let tx = plain_tx_with_default_details::<R, S>(&msg);
            let signed_tx = {
                let TransactionType::Plain {
                    message,
                    key,
                    details,
                } = tx
                else {
                    panic!("The method `plain_tx_with_default_details` should return a plain transaction!");
                };

                TransactionType::<R, S>::sign(message, key, &R::CHAIN_HASH, details, &mut nonces)
            };
            txns.push(signed_tx);
        }

        let start = std::time::Instant::now();
        tokio::time::timeout(
            Duration::from_secs((txns.len() as u64) * 100),
            client.send_txs_to_sequencer(&txns),
        )
        .await??;
        let elapsed = start.elapsed();
        total_txns += txns.len();
        tracing::debug!(id = %worker_id, "Sent {} transactions in {}ms. Current throughput: {}txs per second. Running throughput: {}txs per second", txns.len(), elapsed.as_millis(), (txns.len() * num_workers as usize) as f64 / elapsed.as_secs_f64(), (total_txns * num_workers as usize) as f64 / worker_start.elapsed().as_secs_f64());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args = Args::parse();
    let _guard = sov_modules_rollup_blueprint::logging::initialize_logging();
    let mut worker_set = JoinSet::new();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let reqwest_client = reqwest::ClientBuilder::new()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(60))
        .read_timeout(Duration::from_secs(120))
        .build()?;
    let client = Client::new_with_client(&args.api_url, reqwest_client);

    for i in 0..args.num_workers {
        worker_set.spawn(worker_task(
            client.clone(),
            rx.clone(),
            (i + args.salt) as u128,
            args.num_workers,
            args.runtime,
        ));
    }

    let mut terminate = tokio::signal::unix::signal(SignalKind::terminate())
        .expect("Failed to set up SIGTERM handler");
    let mut quit =
        tokio::signal::unix::signal(SignalKind::quit()).expect("Failed to set up SIGQUIT handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("Received Ctrl+C"),
        _ = terminate.recv() => tracing::info!("Received SIGTERM"),
        _ = quit.recv() => tracing::info!("Received SIGQUIT"),
    }

    tx.send(true)?;
    _ = worker_set.join_all();

    Ok(())
}
