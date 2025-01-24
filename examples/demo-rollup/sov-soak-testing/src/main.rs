use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::Rng;
use setup::{plain_tx_with_default_details, setup_harness, setup_rollup, Setup, TestGenerator, RT};
use sov_bank::CallMessageDiscriminants::Transfer;
use sov_modules_api::prelude::arbitrary::Unstructured;
use sov_modules_api::{CryptoSpec, Runtime as _, Spec};
use sov_test_utils::{TestSpec as S, TransactionType};
use sov_transaction_generator::generators::bank::harness_interface::BankHarness;
use sov_transaction_generator::generators::bank::BankMessageGenerator;
use sov_transaction_generator::generators::basic::BasicModuleRef;
use sov_transaction_generator::interface::rng_utils::get_random_bytes;
use sov_transaction_generator::{Distribution, MessageValidity, Percent};

mod setup;

#[derive(Debug)]
struct Summary {
    txns_sent: usize,
    started_at: Instant,
}

async fn main_loop(summary: &mut Summary) -> Result<(), anyhow::Error> {
    let mut nonces: HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64> =
        Default::default();

    let (rollup, setup) = setup_rollup().await;
    let Setup {
        paymaster: _,
        sequencer: _,
        prover: _,
        genesis_config: _,
    } = setup;
    let client = rollup.client.client;

    let mut rng = rand::thread_rng();
    let random_bytes = get_random_bytes(100_000_000, 1);
    let mut u = &mut Unstructured::new(&random_bytes[..]);
    let bank_harness = BankHarness::new(BankMessageGenerator::<S>::new(
        Distribution::with_equiprobable_values(vec![Transfer]),
        Percent::one_hundred(),
    ));
    let modules: Vec<BasicModuleRef<S, RT>> = vec![Arc::new(bank_harness.clone())];
    let modules = Distribution::with_equiprobable_values(modules);
    let mut generator: TestGenerator<RT> = setup_harness();

    loop {
        let txn_count = rng.gen_range(10..100);
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

        summary.txns_sent += txns.len();

        client.send_txs_to_sequencer(&txns).await?;
        let sleep_ms = rng.gen_range(25..100);

        std::thread::sleep(Duration::from_millis(sleep_ms));
    }
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let _guard = sov_modules_rollup_blueprint::logging::initialize_logging();
    let mut summary = Summary {
        txns_sent: 0,
        started_at: Instant::now(),
    };
    let result = main_loop(&mut summary).await;
    let duration = Instant::now().duration_since(summary.started_at).as_secs();

    match result {
        Ok(_) => println!("Successful summary: {:?}, ran for {:?}", summary, duration),
        Err(e) => println!(
            "Main loop exited with failure, {:?}, runtime, {:?}, error: {}",
            summary,
            duration,
            e.to_string()
        ),
    }

    Ok(())
}
