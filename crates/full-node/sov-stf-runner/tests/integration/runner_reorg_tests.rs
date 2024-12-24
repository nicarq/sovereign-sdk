use std::sync::Arc;

use sov_db::ledger_db::LedgerDb;
use sov_db::storage_manager::NativeStorageManager;
use sov_mock_da::{
    MockAddress, MockBlob, MockBlock, MockBlockHeader, MockDaService, MockDaSpec, MockValidityCond,
    PlannedFork,
};
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::{FullyBakedTx, StateTransitionFunction};
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_state::storage::NativeStorage;
use sov_state::{ArrayWitness, ProverStorage, Storage, StorageRoot};
use sov_test_utils::storage::SimpleStorageManager;
use tempfile::TempDir;

use crate::helpers::hash_stf::{HashStf, S};
use crate::helpers::runner_init::{initialize_runner, InitVariant};

type MockInitVariant = InitVariant<HashStf<MockValidityCond>, MockZkvm, MockZkvm, MockDaService>;

#[tokio::test(flavor = "multi_thread")]
async fn test_simple_reorg_case() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let sequencer_address = MockAddress::new([11u8; 32]);
    let genesis_params = vec![1, 2, 3, 4, 5];

    let main_chain_blobs = vec![
        batch(vec![1, 1, 1, 1]),
        batch(vec![2, 2, 2, 2]),
        batch(vec![3, 3, 3, 3]),
        batch(vec![4, 4, 4, 4]),
    ];
    let fork_blobs = vec![
        batch(vec![13, 13, 13, 13]),
        batch(vec![14, 14, 14, 14]),
        batch(vec![15, 15, 15, 15]),
    ];
    let expected_final_blobs = vec![
        batch(vec![1, 1, 1, 1]),
        batch(vec![2, 2, 2, 2]),
        batch(vec![13, 13, 13, 13]),
        batch(vec![14, 14, 14, 14]),
        batch(vec![15, 15, 15, 15]),
    ];

    let mut da_service = MockDaService::new(sequencer_address)
        .with_finality(4)
        .with_wait_attempts(2);

    let genesis_block = da_service.get_block_at(0).await.unwrap();

    let planned_fork = PlannedFork::new(5, 2, fork_blobs.clone());
    da_service.set_planned_fork(planned_fork).await.unwrap();

    let da_service = Arc::new(da_service);
    for data in main_chain_blobs {
        let fee = da_service.estimate_fee(data.len()).await.unwrap();
        da_service
            .send_transaction(&data, fee)
            .await
            .await
            .unwrap()
            .unwrap();
    }

    let (expected_state_root, _expected_final_root_hash) =
        get_expected_execution_hash_from(&genesis_params, expected_final_blobs);

    let (_expected_committed_state_root, expected_committed_root_hash) =
        get_expected_execution_hash_from(&genesis_params, vec![batch(vec![1, 1, 1, 1])]);

    let init_variant: MockInitVariant = InitVariant::Genesis {
        block: genesis_block,
        genesis_params,
    };

    check_runner(da_service, &tmp_dir, init_variant, expected_state_root).await;

    let committed_root_hash = get_saved_root_hash(tmp_dir.path()).unwrap().unwrap();
    assert_eq!(expected_committed_root_hash.unwrap(), committed_root_hash);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "TBD"]
async fn test_several_reorgs() {}

#[tokio::test(flavor = "multi_thread")]
async fn test_instant_finality_data_stored() -> anyhow::Result<()> {
    let tmp_dir = tempfile::tempdir().unwrap();
    let sequencer_address = MockAddress::new([11u8; 32]);
    let genesis_params = vec![1, 2, 3, 4, 5];

    let da_service = Arc::new(MockDaService::new(sequencer_address).with_wait_attempts(2));

    let genesis_block = da_service.get_block_at(0).await?;
    let fee = da_service.estimate_fee(4).await?;

    let serialized_blob_1 = batch(vec![1, 1, 1, 1]);

    da_service
        .send_transaction(&serialized_blob_1, fee)
        .await
        .await??;

    let serialized_blob_2 = batch(vec![2, 2, 2, 2]);

    da_service
        .send_transaction(&serialized_blob_2, fee)
        .await
        .await??;

    let serialized_blob_3 = batch(vec![3, 3, 3, 3]);

    da_service
        .send_transaction(&serialized_blob_3, fee)
        .await
        .await??;

    let (expected_state_root, expected_root_hash) = get_expected_execution_hash_from(
        &genesis_params,
        vec![serialized_blob_1, serialized_blob_2, serialized_blob_3],
    );

    let init_variant: MockInitVariant = InitVariant::Genesis {
        block: genesis_block,
        genesis_params,
    };

    check_runner(da_service, &tmp_dir, init_variant, expected_state_root).await;

    let saved_root_hash = get_saved_root_hash(tmp_dir.path()).unwrap().unwrap();
    assert_eq!(expected_root_hash.unwrap(), saved_root_hash);
    Ok(())
}

async fn check_runner(
    da_service: Arc<MockDaService>,
    tmpdir: &TempDir,
    init_variant: MockInitVariant,
    expected_state_root: StorageRoot<S>,
) {
    let (mut runner, _test_node) =
        initialize_runner(da_service, tmpdir.path(), init_variant, 1, None).await;
    let before = *runner.get_state_root();
    let end = runner.run_in_process().await;
    // TODO: Subscribe to block notifications and shutdown runner afterwards.
    assert!(end.is_err());
    let after = *runner.get_state_root();

    assert_ne!(before, after);
    assert_eq!(expected_state_root, after);
}

fn get_saved_root_hash(
    path: &std::path::Path,
) -> anyhow::Result<Option<<ProverStorage<S> as Storage>::Root>> {
    let mut storage_manager =
        NativeStorageManager::<MockDaSpec, ProverStorage<S>>::new(path).unwrap();
    let mock_block_header = MockBlockHeader::from_height(1000000);
    let (stf_state, ledger_state) = storage_manager.create_state_for(&mock_block_header)?;

    let ledger_db = LedgerDb::with_reader(ledger_state).unwrap();

    ledger_db
        .get_head_slot()?
        .map(|(number, _)| stf_state.get_root_hash(number.0))
        .transpose()
}

fn get_expected_execution_hash_from(
    genesis_params: &[u8],
    blobs: Vec<Vec<u8>>,
) -> (StorageRoot<S>, Option<<ProverStorage<S> as Storage>::Root>) {
    let blocks: Vec<MockBlock> = blobs
        .into_iter()
        .enumerate()
        .map(|(idx, blob)| MockBlock {
            header: MockBlockHeader::from_height((idx + 1) as u64),
            validity_cond: MockValidityCond::default(),
            batch_blobs: vec![MockBlob::new(
                blob,
                MockAddress::new([11u8; 32]),
                [idx as u8; 32],
            )],
            proof_blobs: Default::default(),
        })
        .collect();

    get_result_from_blocks(genesis_params, &blocks[..])
}

// Returns final data hash and root hash
fn get_result_from_blocks(
    genesis_params: &[u8],
    blocks: &[MockBlock],
) -> (StorageRoot<S>, Option<<ProverStorage<S> as Storage>::Root>) {
    let mut storage_manager = SimpleStorageManager::new();
    let storage = storage_manager.create_storage();

    let stf = HashStf::<MockValidityCond>::new();

    let (genesis_state_root, change_set) = <HashStf<MockValidityCond> as StateTransitionFunction<
        MockZkvm,
        MockZkvm,
        MockDaSpec,
    >>::init_chain(
        &stf,
        &Default::default(),
        &Default::default(),
        storage,
        genesis_params.to_vec(),
    );
    storage_manager.commit(change_set);

    let mut state_root = genesis_state_root;

    let l = blocks.len();

    for block in blocks {
        let mut relevant_blobs = block.as_relevant_blobs();

        let storage = storage_manager.create_storage();
        let result = <HashStf<MockValidityCond> as StateTransitionFunction<
            MockZkvm,
            MockZkvm,
            MockDaSpec,
        >>::apply_slot::<&mut [MockBlob]>(
            &stf,
            &state_root,
            storage,
            ArrayWitness::default(),
            &block.header,
            &block.validity_cond,
            relevant_blobs.as_iters(),
            sov_modules_api::ExecutionContext::Node,
        );

        state_root = result.state_root;
        storage_manager.commit(result.change_set);
    }

    let storage = storage_manager.create_storage();
    let root_hash = storage.get_root_hash(l as u64).ok();
    (state_root, root_hash)
}

fn batch(data: Vec<u8>) -> Vec<u8> {
    borsh::to_vec(&vec![FullyBakedTx { data }]).unwrap()
}
