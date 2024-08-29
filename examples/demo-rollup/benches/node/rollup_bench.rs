#![allow(clippy::float_arithmetic)]

use std::env;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use demo_stf::genesis_config::{create_genesis_config, GenesisPaths};
use demo_stf::runtime::Runtime;
use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockBlock, MockBlockHeader, MockDaSpec};
use sov_modules_stf_blueprint::{GenesisParams, StfBlueprint};
use sov_rng_da_service::RngDaService;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::stf::{ExecutionContext, StateTransitionFunction};
use sov_test_utils::storage::SimpleStorageManager;
use tempfile::TempDir;

type BenchSpec = sov_test_utils::TestSpec;

fn rollup_bench(_bench: &mut Criterion) {
    let genesis_height: u64 = 0u64;
    let mut end_height: u64 = 100u64;
    if let Ok(val) = env::var("BLOCKS") {
        end_height = val.parse().expect("BLOCKS var should be a +ve number");
    }

    let mut c = Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(20));
    let temp_dir = TempDir::new().expect("Unable to create temporary directory");
    let da_service = Arc::new(RngDaService::new());

    let mut storage_manager = SimpleStorageManager::new(temp_dir.path());
    let stf_state = storage_manager.create_storage();

    let stf = StfBlueprint::<
        BenchSpec,
        MockDaSpec,
        Runtime<BenchSpec, MockDaSpec>,
        BasicKernel<BenchSpec, _>,
    >::new();

    let demo_genesis_config = {
        let tests_path: &Path = "../../test-data/genesis/integration-tests".as_ref();
        let rt_params =
            create_genesis_config::<BenchSpec, _>(&GenesisPaths::from_dir(tests_path)).unwrap();

        let kernel_params =
            BasicKernelGenesisConfig::from_path(tests_path.join("chain_state.json")).unwrap();
        GenesisParams {
            runtime: rt_params,
            kernel: kernel_params,
        }
    };

    let (mut current_root, stf_change_set) = stf.init_chain(stf_state, demo_genesis_config);

    storage_manager.commit(stf_change_set);
    // data generation
    let mut blobs = vec![];
    let mut blocks = vec![];
    for height in genesis_height..end_height {
        let filtered_block = MockBlock {
            header: MockBlockHeader::from_height(height),
            validity_cond: Default::default(),
            batch_blobs: Default::default(),
            proof_blobs: Default::default(),
        };
        let blob_txs = da_service.extract_relevant_blobs(&filtered_block);

        blocks.push(filtered_block);
        blobs.push(blob_txs.clone());
    }

    let mut height = 0u64;
    c.bench_function("rollup main stf loop", |b| {
        b.iter(|| {
            let stf_storage = storage_manager.create_storage();
            let filtered_block = &blocks[height as usize];
            let apply_block_result = stf.apply_slot(
                &current_root,
                stf_storage,
                Default::default(),
                &filtered_block.header,
                &filtered_block.validity_cond,
                blobs[height as usize].as_iters(),
                ExecutionContext::Node,
            );
            current_root = apply_block_result.state_root;
            storage_manager.commit(apply_block_result.change_set);
            height += 1;
        });
    });
}

criterion_group!(benches, rollup_bench);
criterion_main!(benches);
