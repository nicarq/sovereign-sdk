#![allow(clippy::float_arithmetic)]

mod datagen;
#[macro_use]
extern crate prettytable;

use std::collections::HashMap;
use std::env;
use std::path::Path;

use demo_stf::genesis_config::{create_genesis_config, GenesisPaths};
use demo_stf::runtime::{GenesisConfig, Runtime};
use prettytable::Table;
use sov_db::storage_manager::NativeChangeSet;
use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockAddress, MockDaService, MockDaSpec};
use sov_modules_api::default_spec::DefaultSpec;
use sov_modules_api::execution_mode::WitnessGeneration;
use sov_modules_api::{CryptoSpecExt, SlotData, Spec, Zkvm};
use sov_modules_stf_blueprint::{GenesisParams, StfBlueprint};
use sov_risc0_adapter::host::Risc0Host;
use sov_risc0_adapter::Risc0Verifier;
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::stf::{ExecutionContext, StateTransitionFunction};
use sov_rollup_interface::zk::{
    StateTransitionWitness, StateTransitionWitnessWithAddress, ZkvmHost,
};
use sov_sp1_adapter::host::SP1Host;
use sov_sp1_adapter::SP1Verifier;
use sov_state::Storage;
use sov_test_utils::storage::SimpleStorageManager;
use tempfile::TempDir;

use crate::datagen::{generate_genesis_config, get_bench_blocks};

const DEFAULT_GENESIS_CONFIG_DIR: &str = "../test-data/genesis/benchmark";

fn print_cycle_averages(metric_map: HashMap<String, (u64, u64)>) {
    let mut metrics_vec: Vec<(String, (u64, u64))> = metric_map
        .iter()
        .map(|(k, (sum, count))| {
            (
                k.clone(),
                (((*sum as f64) / (*count as f64)).round() as u64, *count),
            )
        })
        .collect();

    metrics_vec.sort_by(|a, b| b.1.cmp(&a.1));

    let mut table = Table::new();
    table.add_row(row![
        "Function",
        "Average Cycles",
        "Num Calls",
        "Total Cycles"
    ]);
    for (k, (avg, count)) in metrics_vec {
        table.add_row(row![
            k,
            format!("{}", avg),
            format!("{}", count),
            format!("{}", avg * count)
        ]);
    }
    table.printstd();
}

fn chain_stats(num_blocks: usize, num_blocks_with_txns: usize, num_txns: usize, num_blobs: usize) {
    let mut table = Table::new();
    table.add_row(row!["Total blocks", num_blocks]);
    table.add_row(row!["Blocks with transactions", num_blocks_with_txns]);
    table.add_row(row!["Number of blobs", num_blobs]);
    table.add_row(row!["Total number of transactions", num_txns]);
    table.add_row(row![
        "Average number of transactions per block",
        ((num_txns as f64) / (num_blocks_with_txns as f64)) as u64
    ]);
    table.printstd();
}

type BenchRisc0Spec = DefaultSpec<Risc0Verifier, Risc0Verifier, WitnessGeneration>;

type BenchRisc0STF = StfBlueprint<
    BenchRisc0Spec,
    MockDaSpec,
    Runtime<BenchRisc0Spec, MockDaSpec>,
    BasicKernel<BenchRisc0Spec, MockDaSpec>,
>;

type BenchSP1Spec = DefaultSpec<SP1Verifier, SP1Verifier, WitnessGeneration>;

type BenchSP1STF = StfBlueprint<
    BenchSP1Spec,
    MockDaSpec,
    Runtime<BenchSP1Spec, MockDaSpec>,
    BasicKernel<BenchSP1Spec, MockDaSpec>,
>;

/// Simple enum to select the test mode.
enum BenchMode {
    Risc0,
    SP1,
}

/// Data collected from the bench run.
struct BenchData {
    num_blocks: usize,
    num_blobs: usize,
    num_blocks_with_txns: usize,
    num_total_transactions: usize,
}

/// Log the bench data to the console. Only logs the bench data if the `bench` feature is enabled.
fn log_bench_data(bench_data: BenchData, mode: BenchMode) {
    #[cfg(feature = "bench")]
    {
        let hashmap_guard = sov_cycle_utils::METRICS_HASHMAP.lock().unwrap();

        let mut metric_map = hashmap_guard.clone();
        // Set cycles per block to main counts if in SP1 mode, so that outputs are consistent.
        if let BenchMode::SP1 = mode {
            let main_counts = metric_map.remove("main").unwrap();
            metric_map.insert("Cycles per block".to_owned(), main_counts);
        };

        println!("Number of keys: {}", metric_map.keys().len());
        for key in metric_map.keys() {
            println!("- {}", key);
        }

        let total_cycles = metric_map.get("Cycles per block").unwrap().0;
        println!("\nBlock stats\n");
        chain_stats(
            bench_data.num_blocks,
            bench_data.num_blocks_with_txns,
            bench_data.num_total_transactions,
            bench_data.num_blobs,
        );
        println!("\nCycle Metrics\n");
        print_cycle_averages(metric_map);
        println!("\nTotal cycles consumed for test: {}\n", total_cycles);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Run the risc0 benchmarks
    run(BenchRisc0STF::new(), risc0::MOCK_DA_ELF, BenchMode::Risc0).await?;

    // Clear the global hashmap used for collecting metrics to avoid polluting the SP1 results.
    // Dot it in its own scope to prevent the lock from being held for too long.
    {
        let mut metrics_map = sov_cycle_utils::METRICS_HASHMAP.lock().unwrap();
        metrics_map.drain();
        assert!(metrics_map.is_empty());
    }

    // Run the SP1 benchmarks
    run(BenchSP1STF::new(), &sp1::SP1_GUEST_MOCK_ELF, BenchMode::SP1).await?;
    Ok(())
}

impl BenchZkvm for Risc0Verifier {
    type Host<'a> = Risc0Host<'a>;
    fn from_elf(da_elf: &[u8]) -> Self::Host<'_> {
        Risc0Host::new(da_elf)
    }
}

impl BenchZkvm for SP1Verifier {
    type Host<'a> = SP1Host<'a>;
    fn from_elf(da_elf: &[u8]) -> Self::Host<'_> {
        SP1Host::new(da_elf)
    }
}
trait BenchZkvm: Zkvm {
    type Host<'a>: ZkvmHost;
    fn from_elf(da_elf: &[u8]) -> Self::Host<'_>;
}

async fn run<InnerVm: Zkvm + BenchZkvm, OuterVm: Zkvm, Stf>(
    stf: Stf,
    elf: &[u8],
    bench_mode: BenchMode,
) -> anyhow::Result<()>
where
    InnerVm::CryptoSpec: CryptoSpecExt,
    OuterVm::CryptoSpec: CryptoSpecExt,
    Stf: StateTransitionFunction<
    InnerVm,
    OuterVm,
    MockDaSpec,
    ChangeSet = NativeChangeSet,
    GenesisParams = GenesisParams<GenesisConfig<DefaultSpec<InnerVm, OuterVm, WitnessGeneration>, MockDaSpec>, BasicKernelGenesisConfig<DefaultSpec<InnerVm, OuterVm, WitnessGeneration>, MockDaSpec>>,
    PreState = <DefaultSpec<InnerVm, OuterVm, WitnessGeneration> as Spec>::Storage,
    StateRoot = <<DefaultSpec<InnerVm, OuterVm, WitnessGeneration> as Spec>::Storage as Storage>::Root,
>,
{
    let genesis_conf_dir = env::var("GENESIS_CONFIG_DIR").unwrap_or_else(|_| {
        println!("GENESIS_CONFIG_DIR not set, using default");
        String::from(DEFAULT_GENESIS_CONFIG_DIR)
    });

    let mut num_blocks = 0;
    let mut num_blobs = 0;
    let mut num_blocks_with_txns = 0;
    let mut num_total_transactions = 0;

    let temp_dir = TempDir::new().expect("Unable to create temporary directory");
    let da_service = MockDaService::new(MockAddress::default());

    let mut storage_manager = SimpleStorageManager::new(temp_dir.path());

    generate_genesis_config(genesis_conf_dir.as_str())?;

    let genesis_config = {
        let rt_params = create_genesis_config::<DefaultSpec<InnerVm, OuterVm, WitnessGeneration>, _>(
            &GenesisPaths::from_dir(genesis_conf_dir.as_str()),
        )?;

        let kernel_params = BasicKernelGenesisConfig::from_path(
            Path::new(genesis_conf_dir.as_str()).join("chain_state.json"),
        )?;
        GenesisParams {
            runtime: rt_params,
            kernel: kernel_params,
        }
    };

    println!("Starting from empty storage, initializing chain");
    let stf_state = storage_manager.create_storage();
    let (mut prev_state_root, stf_changes) = stf.init_chain(stf_state, genesis_config);
    storage_manager.commit(stf_changes);

    // TODO: Fix this with genesis logic.
    let blocks = get_bench_blocks().await?;

    for filtered_block in blocks {
        num_blocks += 1;
        let mut host = InnerVm::from_elf(elf);

        let height = filtered_block.header().height();
        println!(
            "Requesting data for height {} and prev_state_root 0x{}",
            height,
            hex::encode(prev_state_root.root_hash())
        );
        let (mut relevant_blobs, relevant_proofs) = da_service
            .extract_relevant_blobs_with_proof(&filtered_block)
            .await;

        if !relevant_blobs.batch_blobs.is_empty() {
            num_blobs += relevant_blobs.batch_blobs.len();
        }

        let stf_state = storage_manager.create_storage();

        let result = stf.apply_slot(
            &prev_state_root,
            stf_state,
            Default::default(),
            filtered_block.header(),
            &filtered_block.validity_condition(),
            relevant_blobs.as_iters(),
            ExecutionContext::Node,
        );

        for r in result.batch_receipts {
            let num_tx = r.tx_receipts.len();
            num_total_transactions += num_tx;
            if num_tx > 0 {
                num_blocks_with_txns += 1;
            }
        }

        let data = StateTransitionWitness::<
            <Stf as StateTransitionFunction<InnerVm, OuterVm, MockDaSpec>>::StateRoot,
            <Stf as StateTransitionFunction<InnerVm, OuterVm, MockDaSpec>>::Witness,
            MockDaSpec,
        > {
            initial_state_root: prev_state_root,
            da_block_header: filtered_block.header,
            relevant_proofs,
            witness: result.witness,
            relevant_blobs,
            final_state_root: result.state_root,
        };

        let data = StateTransitionWitnessWithAddress {
            stf_witness: data,
            prover_address: MockAddress::default(),
        };

        host.add_hint(data);

        println!("Skipping prover at block {height} to capture cycle counts\n");
        let _receipt = host.run(false).expect("Prover should run successfully");
        println!("==================================================\n");
        prev_state_root = result.state_root;
        storage_manager.commit(result.change_set);
    }

    log_bench_data(
        BenchData {
            num_blocks,
            num_blobs,
            num_blocks_with_txns,
            num_total_transactions,
        },
        bench_mode,
    );

    Ok(())
}
