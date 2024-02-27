use sov_mock_da::{MockBlockHeader, MockDaSpec, MockValidityCond};
use sov_mock_zkvm::MockZkVerifier;
use sov_rollup_interface::rpc::LedgerRpcProvider;
use sov_rollup_interface::services::da::DaService;
use sov_stf_runner::InitVariant;

mod helpers;
use helpers::hash_stf::HashStf;
use helpers::runner_init::initialize_runner;

type MockInitVariant = InitVariant<HashStf<MockValidityCond>, MockZkVerifier, MockDaSpec>;

#[tokio::test]
async fn fetch_aggregated_proof_test() -> Result<(), anyhow::Error> {
    let tmpdir = tempfile::tempdir().unwrap();
    let genesis_params = vec![1, 2, 3, 4, 5];
    let init_variant: MockInitVariant = InitVariant::Genesis {
        block_header: MockBlockHeader::from_height(0),
        genesis_params,
    };

    let (mut runner, ledger_db, da, vm) = initialize_runner(tmpdir.path(), init_variant);
    let mut slot_sub = ledger_db.subscribe_slots().unwrap();

    tokio::spawn(async move {
        runner.run_in_process().await.unwrap();
    });

    {
        da.send_transaction(&[1, 2, 3]).await?;
        slot_sub.recv().await?;
        vm.make_proof();
        da.wait_for_aggregated_proof_in_da().await;
    }

    {
        da.send_transaction(&[1, 2, 3]).await?;
        slot_sub.recv().await?;
    }

    let proof_from_db = ledger_db.get_latest_aggregated_proof()?.unwrap();
    let info = proof_from_db.proof.info();
    assert_eq!(2, info.initial_slot_number);
    assert_eq!(2, info.final_slot_number);

    Ok(())
}
