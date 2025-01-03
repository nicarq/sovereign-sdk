//! CLI utilities for benchmark generation.

use clap::{Args, Parser, Subcommand};
use sov_benchmarks::generator::{Benchmark, DEFAULT_RANDOMIZATION_BUFFER_SIZE};
use sov_transaction_generator::generators::basic::BasicModuleRef;
use sov_transaction_generator::{Distribution, MessageValidity};

use crate::{RT, S};

const DEFAULT_GENERATION_PATH: &str = "./src/bench_files/generated/";

const DEFAULT_INITIAL_SEED: u128 = 0;

const NUM_SLOTS: u64 = 5;

/// Benchmark parameters to define a small benchmark
pub const SMALL_BENCH_PARAMS: BenchCLICustomArgs = BenchCLICustomArgs {
    min_txs_per_batch: 100,
    max_txs_per_batch: 1000,
    min_batches_per_slot: 1,
    max_batches_per_slot: 5,
    randomization_buffer_size: DEFAULT_RANDOMIZATION_BUFFER_SIZE / 10,
    max_value_setter_vec_len: 1000,
};

/// Benchmark parameters to define a standard benchmark
pub const STANDARD_BENCH_PARAMS: BenchCLICustomArgs = BenchCLICustomArgs {
    min_txs_per_batch: 1000,
    max_txs_per_batch: 5000,
    min_batches_per_slot: 10,
    max_batches_per_slot: 20,
    randomization_buffer_size: DEFAULT_RANDOMIZATION_BUFFER_SIZE,
    max_value_setter_vec_len: 10_000,
};

/// Benchmark parameters to define a large benchmark
pub const LARGE_BENCH_PARAMS: BenchCLICustomArgs = BenchCLICustomArgs {
    min_txs_per_batch: 5000,
    max_txs_per_batch: 10_000,
    min_batches_per_slot: 10,
    max_batches_per_slot: 50,
    randomization_buffer_size: 10 * DEFAULT_RANDOMIZATION_BUFFER_SIZE,
    max_value_setter_vec_len: 100_000,
};

/// This program automatically generates benchmarks using the `sov-transaction-generator`. Please note that:
/// - The files that are generated systematically come from the [`basic_benches`]. In the future we may want to add more control over the benchmark to generate.
/// - The path argument should either define a relative path from the root of this crate (ie `sov-benchmarks`), or any absolute path.
#[derive(Parser, Debug)]
#[command(
    version,
    author,
    about = "This program automatically generates benchmarks using the `sov-transaction-generator`."
)]
pub struct BenchCLI {
    /// The base path to store the bench files. It must be a folder that currently exists.
    #[arg(short, long, default_value_t=DEFAULT_GENERATION_PATH.to_string())]
    pub path: String,

    /// Number of slots to execute as part of the benchmark.
    #[arg(short, long, default_value_t=NUM_SLOTS)]
    pub slots: u64,

    /// The seed value used for non-determinism.
    #[arg(long, default_value_t=DEFAULT_INITIAL_SEED)]
    pub seed: u128,

    /// Defines the benchmark size.
    #[command(subcommand)]
    pub size: BenchSize,
}

impl BenchCLI {
    /// Parses the size argument into a [`BenchCLICustomArgs`]
    pub fn parse_size(&self) -> &BenchCLICustomArgs {
        match &self.size {
            BenchSize::Small => &SMALL_BENCH_PARAMS,
            BenchSize::Standard => &STANDARD_BENCH_PARAMS,
            BenchSize::Large => &LARGE_BENCH_PARAMS,
            BenchSize::Custom(params) => params,
        }
    }
}

/// The different sizes available for benchmarking.
#[derive(Subcommand, Debug)]
pub enum BenchSize {
    /// Runs a small benchmark. Ie, with [`SMALL_BENCH_PARAMS`] values.
    Small,
    /// Runs a standard benchmark. Ie, with [`STANDARD_BENCH_PARAMS`] values.
    Standard,
    /// Runs a large benchmark. Ie, with [`LARGE_BENCH_PARAMS`] values.
    Large,
    /// Generates a benchmark with custom arguments.
    Custom(BenchCLICustomArgs),
}

/// Custom benchmark arguments.
/// The range parameters ([`BenchCLICustomArgs::min_txs_per_batch`], [`BenchCLICustomArgs::max_txs_per_batch`], [`BenchCLICustomArgs::min_batches_per_slot`], [`BenchCLICustomArgs::max_batches_per_slot`]) define closed ranges, ie of the form `min_val..=max_val`
#[derive(Args, Debug)]
pub struct BenchCLICustomArgs {
    /// Minimum number of transactions to include per batch, included.
    pub min_txs_per_batch: u64,

    /// Maximum number of transactions to include per batch, included.
    pub max_txs_per_batch: u64,

    /// Minimum number of batches to include per slot, included.
    pub min_batches_per_slot: u64,

    /// Maximum number of batches to include per slot, included.
    pub max_batches_per_slot: u64,

    /// The randomization buffer size.
    pub randomization_buffer_size: u64,

    /// The maximum value setter vector length.
    pub max_value_setter_vec_len: usize,
}

impl BenchCLICustomArgs {
    /// Generates a new benchmark with custom params
    pub fn new_benchmark_with_params(
        &self,
        name: String,
        slots: u64,
        seed: u128,
        module_distribution: Distribution<BasicModuleRef<S, RT>>,
        validity_distribution: Distribution<MessageValidity>,
    ) -> Benchmark<S> {
        Benchmark {
            name,
            module_distribution,
            message_validity_distribution: validity_distribution,
            transactions_per_batch_range: self.min_txs_per_batch..=self.max_txs_per_batch,
            batches_per_slot_range: self.min_batches_per_slot..=self.max_batches_per_slot,
            number_of_slots: slots,
            initial_seed: seed,
            initial_randomization_buffer_size: self.randomization_buffer_size,
        }
    }
}
