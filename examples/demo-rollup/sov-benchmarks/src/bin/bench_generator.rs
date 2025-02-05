//! This file generate harness files and stores them in the harnesses `generated` folder.

use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use demo_stf::runtime::Runtime;
use sov_benchmarks::bench_generator::cli::{BenchCLI, BenchCLICustomArgs};
use sov_benchmarks::bench_generator::Benchmark;
use sov_benchmarks::BenchSpec;
use sov_metrics::timestamp;
use sov_modules_api::{CryptoSpec, PrivateKey, Spec};
use sov_risc0_adapter::Risc0;
use sov_test_utils::initialize_logging;
use sov_test_utils::runtime::sov_bank::CallMessageDiscriminants as BankDiscriminants;
use sov_test_utils::runtime::sov_value_setter::CallMessageDiscriminants as ValueSetterDiscriminants;
use sov_transaction_generator::generators::bank::harness_interface::BankHarness;
use sov_transaction_generator::generators::bank::BankMessageGenerator;
use sov_transaction_generator::generators::basic::BasicModuleRef;
use sov_transaction_generator::generators::value_setter::{
    ValueSetterHarness, ValueSetterMessageGenerator,
};
use sov_transaction_generator::{Distribution, MessageValidity, Percent};
use tracing::{info, info_span};

type S = BenchSpec<Risc0>;
type RT = Runtime<S>;

/// A basic benchmark that tries to emulate variable behaviors from transaction execution.
pub fn basic_benches(params: &BenchCLICustomArgs, slots: u64, seed: u128) -> Vec<Benchmark<S>> {
    let value_setter_admin = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate();

    vec![
        {
            let bench_module: BasicModuleRef<S, RT> =
                Arc::new(BankHarness::new(BankMessageGenerator::new(
                    Distribution::with_equiprobable_values(vec![BankDiscriminants::Transfer]),
                    Percent::one_hundred(),
                )));
            params.new_benchmark_with_params(
                "bank_transfers_100_percent_address_creation".to_string(),
                slots,
                seed,
                Distribution::with_equiprobable_values(vec![bench_module]),
                Distribution::with_equiprobable_values(vec![
                    MessageValidity::Valid,
                    // TODO(@theochap) - reactivate once we have a way to send invalid transactions
                    // to DA through sequencers.
                    // MessageValidity::Invalid,
                ]),
                value_setter_admin.clone(),
            )
        },
        {
            let bench_module: BasicModuleRef<S, RT> =
                Arc::new(BankHarness::new(BankMessageGenerator::new(
                    Distribution::with_equiprobable_values(vec![BankDiscriminants::Transfer]),
                    Percent::fifty(),
                )));
            params.new_benchmark_with_params(
                "bank_transfers".to_string(),
                slots,
                seed,
                Distribution::with_equiprobable_values(vec![bench_module]),
                Distribution::with_equiprobable_values(vec![
                    MessageValidity::Valid,
                    // TODO(@theochap) - reactivate once we have a way to send invalid transactions
                    // to DA through sequencers.
                    // MessageValidity::Invalid,
                ]),
                value_setter_admin.clone(),
            )
        },
        {
            let bench_module: BasicModuleRef<S, RT> =
                Arc::new(BankHarness::new(BankMessageGenerator::new(
                    Distribution::with_equiprobable_values(vec![
                        BankDiscriminants::Transfer,
                        BankDiscriminants::CreateToken,
                        BankDiscriminants::Mint,
                        BankDiscriminants::Burn,
                        BankDiscriminants::Freeze,
                    ]),
                    Percent::fifty(),
                )));

            params.new_benchmark_with_params(
                "bank_messages".to_string(),
                slots,
                seed,
                Distribution::with_equiprobable_values(vec![bench_module]),
                Distribution::with_equiprobable_values(vec![
                    MessageValidity::Valid,
                    // TODO(@theochap) - reactivate once we have a way to send invalid transactions
                    // to DA through sequencers.
                    // MessageValidity::Invalid,
                ]),
                value_setter_admin.clone(),
            )
        },
        {
            let bench_module: BasicModuleRef<S, RT> =
                Arc::new(ValueSetterHarness::new(ValueSetterMessageGenerator::new(
                    Distribution::with_equiprobable_values(vec![
                        ValueSetterDiscriminants::SetValue,
                    ]),
                    params.max_value_setter_vec_len,
                    value_setter_admin.clone(),
                )));

            params.new_benchmark_with_params(
                "value_setter_set_value".to_string(),
                slots,
                seed,
                Distribution::with_equiprobable_values(vec![bench_module]),
                Distribution::with_equiprobable_values(vec![
                    MessageValidity::Valid,
                    // TODO(@theochap) - reactivate once we have a way to send invalid transactions
                    // to DA through sequencers.
                    // MessageValidity::Invalid,
                ]),
                value_setter_admin.clone(),
            )
        },
        {
            let bench_module: BasicModuleRef<S, RT> =
                Arc::new(ValueSetterHarness::new(ValueSetterMessageGenerator::new(
                    Distribution::with_equiprobable_values(vec![
                        ValueSetterDiscriminants::SetValue,
                        ValueSetterDiscriminants::SetManyValues,
                    ]),
                    params.max_value_setter_vec_len,
                    value_setter_admin.clone(),
                )));
            params.new_benchmark_with_params(
                "value_setter_messages".to_string(),
                slots,
                seed,
                Distribution::with_equiprobable_values(vec![bench_module]),
                Distribution::with_equiprobable_values(vec![
                    MessageValidity::Valid,
                    // TODO(@theochap) - reactivate once we have a way to send invalid transactions
                    // to DA through sequencers.
                    // MessageValidity::Invalid,
                ]),
                value_setter_admin.clone(),
            )
        },
        {
            let bank_bench_module: BasicModuleRef<S, RT> =
                Arc::new(BankHarness::new(BankMessageGenerator::new(
                    Distribution::with_equiprobable_values(vec![BankDiscriminants::Transfer]),
                    Percent::fifty(),
                )));
            let value_setter_bench_module: BasicModuleRef<S, RT> =
                Arc::new(ValueSetterHarness::new(ValueSetterMessageGenerator::new(
                    Distribution::with_equiprobable_values(vec![
                        ValueSetterDiscriminants::SetValue,
                        ValueSetterDiscriminants::SetManyValues,
                    ]),
                    params.max_value_setter_vec_len,
                    value_setter_admin.clone(),
                )));
            params.new_benchmark_with_params(
                "mix_bank_transfers_value_setter".to_string(),
                slots,
                seed,
                Distribution::with_equiprobable_values(vec![
                    bank_bench_module,
                    value_setter_bench_module,
                ]),
                Distribution::with_equiprobable_values(vec![
                    MessageValidity::Valid,
                    // TODO(@theochap) - reactivate once we have a way to send invalid transactions
                    // to DA through sequencers.
                    // MessageValidity::Invalid,
                ]),
                value_setter_admin.clone(),
            )
        },
        {
            let bank_bench_module: BasicModuleRef<S, RT> =
                Arc::new(BankHarness::new(BankMessageGenerator::new(
                    Distribution::with_equiprobable_values(vec![
                        BankDiscriminants::Transfer,
                        BankDiscriminants::CreateToken,
                        BankDiscriminants::Mint,
                        BankDiscriminants::Burn,
                        BankDiscriminants::Freeze,
                    ]),
                    Percent::fifty(),
                )));
            let value_setter_bench_module: BasicModuleRef<S, RT> =
                Arc::new(ValueSetterHarness::new(ValueSetterMessageGenerator::new(
                    Distribution::with_equiprobable_values(vec![
                        ValueSetterDiscriminants::SetValue,
                        ValueSetterDiscriminants::SetManyValues,
                    ]),
                    params.max_value_setter_vec_len,
                    value_setter_admin.clone(),
                )));

            params.new_benchmark_with_params(
                "complete_bank_value_setter".to_string(),
                slots,
                seed,
                Distribution::with_equiprobable_values(vec![
                    bank_bench_module,
                    value_setter_bench_module,
                ]),
                Distribution::with_equiprobable_values(vec![
                    MessageValidity::Valid,
                    // TODO(@theochap) - reactivate once we have a way to send invalid transactions
                    // to DA through sequencers.
                    // MessageValidity::Invalid,
                ]),
                value_setter_admin.clone(),
            )
        },
    ]
}

#[tokio::main]
async fn main() {
    let params = BenchCLI::parse();

    // If the env var is not set, then we set it to the default value.
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "warn,error,bench_generator=info");
    }

    initialize_logging();

    info_span!("bench_generator");

    let args = params.parse_size();

    let benchmarks = basic_benches(args, params.slots, params.seed);

    let generation_base_path = PathBuf::from(params.path);

    let mut joinset = tokio::task::JoinSet::new();

    for benchmark in benchmarks {
        let generation_base_path_cloned = generation_base_path.clone();
        joinset.spawn(async move {
            let mut bench_with_extension = benchmark.name.clone();
            let bench_stamp = timestamp();
            bench_with_extension.push_str(&format!("_{bench_stamp}.bin"));

            info!(bench = benchmark.name, "Generating benchmark...");

            let path = generation_base_path_cloned.join(bench_with_extension);
            let path_str = path
                .to_str()
                .expect("Path should be well encoded")
                .to_string();

            let file = File::create(path).unwrap_or_else(|_| {
                panic!(
                    "Impossible to generate a file at path {:?}, please ensure the path is valid",
                    path_str,
                )
            });

            let begin_stamp = timestamp();
            benchmark
                .generate_and_write_benchmark_messages(&mut BufWriter::new(file))
                .unwrap_or_else(|_| {
                    panic!(
                        "Impossible to generate benchmark messages for the benchmark {:?}",
                        benchmark.name
                    )
                });
            let end_stamp = timestamp();

            let exec_duration = Duration::from_nanos((end_stamp - begin_stamp) as u64);

            info!(
                bench = benchmark.name,
                generation_duration = format!(
                    "{}.{}s",
                    exec_duration.as_secs(),
                    exec_duration.subsec_millis()
                ),
                "Generation complete...",
            );
        });
    }

    joinset.join_all().await;
}
