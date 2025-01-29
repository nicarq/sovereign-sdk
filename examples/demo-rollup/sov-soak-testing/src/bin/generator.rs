use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use rand::Rng;
use sov_api_spec::Client;
use sov_bank::CallMessageDiscriminants::Transfer;
use sov_modules_api::prelude::arbitrary::Unstructured;
use sov_modules_api::prelude::tracing;
use sov_modules_api::{CryptoSpec, Runtime as _, Spec};
use sov_soak_testing::{plain_tx_with_default_details, setup_harness, TestGenerator, RT};
use sov_test_utils::{TestSpec as S, TransactionType};
use sov_transaction_generator::generators::bank::harness_interface::BankHarness;
use sov_transaction_generator::generators::bank::BankMessageGenerator;
use sov_transaction_generator::generators::basic::BasicModuleRef;
use sov_transaction_generator::interface::rng_utils::get_random_bytes;
use sov_transaction_generator::{Distribution, MessageValidity, Percent};
use tokio::signal::unix::SignalKind;
use tokio::sync::watch::Receiver;
use tokio::task::JoinSet;
// mod setup;

#[derive(Parser)]
struct Args {
    #[arg(short, long, default_value = "http://localhost:12346")]
    /// The URL of the rollup node to connect to. Defaults to http://localhost:12346.
    api_url: String,

    #[arg(short, long, default_value = "5")]
    /// The number of workers to spawn - this controls the number of concurrent transactions. Defaults to 5.
    num_workers: u32,

    #[arg(short, long, default_value = "0")]
    /// The salt to use for RNG. Use this value if you're restarting the generator and want to ensure that the generated
    /// transactions don't overlap with the previous run.
    salt: u32,
}

async fn worker_task(
    client: sov_api_spec::Client,
    rx: Receiver<bool>,
    worker_id: u128,
) -> anyhow::Result<()> {
    if let Err(e) = worker_task_inner(client, rx, worker_id).await {
        tracing::error!("Worker task {worker_id} failed: {}", e);
        std::process::exit(1);
    }
    Ok(())
}

async fn worker_task_inner(
    client: sov_api_spec::Client,
    rx: Receiver<bool>,
    worker_id: u128,
) -> anyhow::Result<()> {
    let mut nonces: HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64> =
        Default::default();

    let random_bytes = get_random_bytes(100_000_000, worker_id);
    let mut u = &mut Unstructured::new(&random_bytes[..]);
    let bank_harness = BankHarness::new(BankMessageGenerator::<S>::new(
        Distribution::with_equiprobable_values(vec![Transfer]),
        Percent::one_hundred(),
    ));
    let modules: Vec<BasicModuleRef<S, RT>> = vec![Arc::new(bank_harness.clone())];
    let modules = Distribution::with_equiprobable_values(modules);
    let mut generator: TestGenerator<RT> = setup_harness(worker_id);

    while !*rx.borrow() {
        let txn_count = {
            // rng must fall out of scope before awaiting anything so this fn is Send
            let mut rng = rand::thread_rng();

            // Do this at the start so we add some jitter to initial API requests
            let sleep_ms = rng.gen_range(25..100);
            std::thread::sleep(Duration::from_millis(sleep_ms));

            rng.gen_range(10..100)
        };
        tracing::debug!(id = %worker_id, "Generating {} transactions", txn_count);

        let mut txns = vec![];
        for _ in 0..txn_count {
            let validity = Distribution::with_equiprobable_values(vec![MessageValidity::Valid]);
            let validity = validity.select_value(&mut u).unwrap();
            let msg = generator.generate(&modules, validity.clone());
            let tx = plain_tx_with_default_details::<RT>(&msg);
            let signed_tx = {
                let TransactionType::Plain {
                    message,
                    key,
                    details,
                } = tx
                else {
                    panic!("The method `plain_tx_with_default_details` should return a plain transaction!");
                };

                TransactionType::<RT, S>::sign(message, key, &RT::CHAIN_HASH, details, &mut nonces)
            };
            txns.push(signed_tx);
        }

        tokio::time::timeout(
            Duration::from_secs(txns.len().try_into().unwrap()),
            client.send_txs_to_sequencer(&txns),
        )
        .await??;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args = Args::parse();
    let _guard = sov_modules_rollup_blueprint::logging::initialize_logging();
    let mut worker_set = JoinSet::new();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let client = Client::new(&args.api_url);

    for i in 0..args.num_workers {
        worker_set.spawn(worker_task(
            client.clone(),
            rx.clone(),
            (i + args.salt) as u128,
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
