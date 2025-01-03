//! This file generate harness files and stores them in the harnesses `generated` folder.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use cli::{BenchCLI, BenchCLICustomArgs};
use demo_stf::runtime::Runtime;
use sov_benchmarks::generator::Benchmark;
use sov_benchmarks::BenchSpec;
use sov_risc0_adapter::Risc0;
use sov_test_utils::runtime::sov_bank::CallMessageDiscriminants as BankDiscriminants;
use sov_test_utils::runtime::sov_value_setter::CallMessageDiscriminants as ValueSetterDiscriminants;
use sov_transaction_generator::generators::bank::harness_interface::BankHarness;
use sov_transaction_generator::generators::bank::BankMessageGenerator;
use sov_transaction_generator::generators::basic::BasicModuleRef;
use sov_transaction_generator::generators::value_setter::{
    ValueSetterHarness, ValueSetterMessageGenerator,
};
use sov_transaction_generator::{Distribution, MessageValidity, Percent};
type S = BenchSpec<Risc0>;
type RT = Runtime<S>;

/// Defines cli utilities
pub mod cli;

/// A basic benchmark that tries to emulate variable behaviors from transaction execution.
pub fn basic_benches(params: &BenchCLICustomArgs, slots: u64, seed: u128) -> Vec<Benchmark<S>> {
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
                    MessageValidity::Invalid,
                ]),
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
                    MessageValidity::Invalid,
                ]),
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
                    MessageValidity::Invalid,
                ]),
            )
        },
        {
            let bench_module: BasicModuleRef<S, RT> =
                Arc::new(ValueSetterHarness::new(ValueSetterMessageGenerator::new(
                    Distribution::with_equiprobable_values(vec![
                        ValueSetterDiscriminants::SetValue,
                    ]),
                    params.max_value_setter_vec_len,
                )));

            params.new_benchmark_with_params(
                "value_setter_set_value".to_string(),
                slots,
                seed,
                Distribution::with_equiprobable_values(vec![bench_module]),
                Distribution::with_equiprobable_values(vec![
                    MessageValidity::Valid,
                    MessageValidity::Invalid,
                ]),
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
                )));
            params.new_benchmark_with_params(
                "value_setter_messages".to_string(),
                slots,
                seed,
                Distribution::with_equiprobable_values(vec![bench_module]),
                Distribution::with_equiprobable_values(vec![
                    MessageValidity::Valid,
                    MessageValidity::Invalid,
                ]),
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
                    MessageValidity::Invalid,
                ]),
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
                    MessageValidity::Invalid,
                ]),
            )
        },
    ]
}

fn main() {
    let params = BenchCLI::parse();

    let args = params.parse_size();

    let benchmarks = basic_benches(args, params.slots, params.seed);

    let generation_base_path = PathBuf::from(params.path);

    for benchmark in benchmarks {
        let mut bench_with_extension = benchmark.name.clone();
        bench_with_extension.push_str(".bin");

        println!("Generating benchmark {}...", benchmark.name);

        let path = generation_base_path.join(bench_with_extension);
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

        benchmark
            .generate_and_write_benchmark_messages(&mut BufWriter::new(file))
            .unwrap_or_else(|_| {
                panic!(
                    "Impossible to generate benchmark messages for the benchmark {:?}",
                    benchmark.name
                )
            });
    }
}
