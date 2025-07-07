use std::sync::Arc;
use std::time::Duration;

use sov_mock_da::BlockProducingConfig;
use sov_modules_api::Runtime;
use sov_modules_stf_blueprint::GenesisParams;
use sov_paymaster::PaymasterConfig;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::test_rollup::TestRollup;
use sov_test_utils::{
    TestSpec, TestUser, TEST_BLOB_PROCESSING_TIMEOUT, TEST_FINALIZATION_BLOCKS, TEST_MAX_BATCH_SIZE,
};
use sov_value_setter::ValueSetterConfig;

#[allow(unused_imports)]
use crate::preferred_end_to_end::{
    run_action_against_test_rollup, run_actions_against_test_rollup,
    setup_test_rollup_with_initial_state, FailureReason, InvalidGeneration, TestBlueprint,
    TestRuntime, TestState, TestingAction,
};
use crate::utils::{new_test_rollup, tempdir_inside_codebase_dir, MAX_BATCH_EXECUTION_TIME_MILLIS};

/// Returns (master, replicas, tempdir, admin).
///
/// Master is None if postgres isn't supported (then replica testing is not possible).
///
/// Each element of replicas is guaranteed to be Some(). It's an option so tests can take()
/// ownership of the TestRollup out of the vec temporarily.
///
/// Tempdir must be kept in scope because it's the parent dir for all the datadirs of each rollup,
/// and there's no builder to keep it in scope.
async fn create_test_rollups(
    num_replicas: u64,
) -> (
    Option<TestRollup<TestBlueprint>>,
    Vec<Option<TestRollup<TestBlueprint>>>,
    Arc<tempfile::TempDir>,
    TestUser<TestSpec>,
) {
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let admin = genesis_config.additional_accounts()[0].clone();

    let rt_genesis_config =
        <TestRuntime<TestSpec> as Runtime<TestSpec>>::GenesisConfig::from_minimal_config(
            genesis_config.into(),
            ValueSetterConfig {
                admin: admin.address(),
            },
            (),
            PaymasterConfig::default(),
            (),
        );

    let genesis_params = GenesisParams {
        runtime: rt_genesis_config.clone(),
    };

    let dir = tempdir_inside_codebase_dir();

    let test_rollups = new_test_rollup::<TestRuntime<TestSpec>>(
<<<<<<< HEAD
            dir.clone(),
            genesis_params
                .runtime
                .sequencer_registry
                .sequencer_config
                .seq_da_address,
            genesis_params,
            3,
            0,
            true,
            TEST_MAX_BATCH_SIZE,
            BlockProducingConfig::Manual,
            None,
            TEST_BLOB_PROCESSING_TIMEOUT,
            num_replicas,
            MAX_BATCH_EXECUTION_TIME_MILLIS,
            None,
            TEST_FINALIZATION_BLOCKS,
        )
        .await;
=======
        dir.clone(),
        genesis_params.runtime.sequencer_registry.seq_da_address,
        genesis_params,
        3,
        0,
        true,
        TEST_MAX_BATCH_SIZE,
        BlockProducingConfig::Manual,
        None,
        TEST_BLOB_PROCESSING_TIMEOUT,
        num_replicas,
        MAX_BATCH_EXECUTION_TIME_MILLIS,
        None,
    )
    .await;
>>>>>>> fmt

    let Some(test_rollups) = test_rollups else {
        return (None, vec![], dir, admin);
    };

    let mut test_rollups = test_rollups.into_iter();

    // Identify initial master and replicas
    let (master, replicas) = identify_master_and_replicas(
        test_rollups.next().unwrap(),
        test_rollups.map(Some).collect(),
    )
    .await
    .unwrap();

    (Some(master), replicas, dir, admin)
}

type MasterAndReplicasAndState = (
    TestRollup<TestBlueprint>,
    Vec<Option<TestRollup<TestBlueprint>>>,
    TestState,
);
async fn test_actions_against_replicas(
    admin: &TestUser<TestSpec>,
    (master, mut replicas, state): MasterAndReplicasAndState,
    actions: Vec<TestingAction>,
) -> MasterAndReplicasAndState {
    let (master, mut state) =
        run_actions_against_test_rollup(actions, master, &admin.clone(), state).await;

    // Ensure replicas have processed the database changes
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify state synchronization across all replicas
    for replica_opt in &mut replicas {
        let replica = replica_opt.take().unwrap();
        let updated_replica = run_action_against_test_rollup(
            replica,
            &admin.private_key,
            TestingAction::QuerySetValue,
            &mut state,
        )
        .await
        .unwrap();
        *replica_opt = Some(updated_replica);
    }
    (master, replicas, state)
}
async fn restart_replica(
    admin: &TestUser<TestSpec>,
    mut replicas: Vec<Option<TestRollup<TestBlueprint>>>,
    test_state: &mut TestState,
    index: usize,
) -> Vec<Option<TestRollup<TestBlueprint>>> {
    let replica = replicas[index].take().unwrap();
    let replica = run_action_against_test_rollup(
        replica,
        admin.private_key(),
        TestingAction::Restart,
        test_state,
    )
    .await
    .unwrap();
    replicas[index] = Some(replica);
    replicas
}

/// Helper function to identify which node is the master after failover.
/// Takes the old master and replicas, checks all nodes, and returns (new_master, new_replicas).
/// Asserts that exactly one node is master.
/// Returns replicas as Vec<Option<TestRollup>> to allow easy temporary ownership with take().
async fn identify_master_and_replicas(
    old_master: TestRollup<TestBlueprint>,
    old_replicas: Vec<Option<TestRollup<TestBlueprint>>>,
) -> anyhow::Result<(
    TestRollup<TestBlueprint>,
    Vec<Option<TestRollup<TestBlueprint>>>,
)> {
    // Collect all nodes to check (filter out None replicas)
    let mut all_nodes = vec![old_master];
    all_nodes.extend(old_replicas.into_iter().flatten());

    let mut master_idx = None;
    let mut master_count = 0;

    for (i, node) in all_nodes.iter().enumerate() {
        let is_master = node.api_client.is_master().await?.into_inner().data;
        if is_master {
            master_idx = Some(i);
            master_count += 1;
        }
    }

    assert_eq!(
        master_count, 1,
        "Expected exactly one master, found {master_count}",
    );
    let master_idx = master_idx.expect("No master found");

    let master = all_nodes.remove(master_idx);
    let replicas = all_nodes.into_iter().map(Some).collect();

    Ok((master, replicas))
}

#[tokio::test(flavor = "multi_thread")]
async fn test_master_election() {
    let (Some(master), mut replicas, _tempdir, admin) = create_test_rollups(4).await else {
        return;
    };

    // Initial setup with state
    let (mut master, mut _state) = setup_test_rollup_with_initial_state(master, &admin).await;

    for iteration in 1..=4 {
        let old_master_node_id = master.api_client.node_id().await.unwrap().into_inner().data;

        // Shutdown current master and get builder for restart
        let master_builder = master.shutdown().await.unwrap();

        // Wait for failover to occur
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Restart the old master as a replica
        let old_master = master_builder.start().await.unwrap();

        // Find new master and verify it's different from the old one
        let (new_master, new_replicas) = identify_master_and_replicas(old_master, replicas)
            .await
            .unwrap();
        let new_master_node_id = new_master
            .api_client
            .node_id()
            .await
            .unwrap()
            .into_inner()
            .data;

        assert_ne!(
            old_master_node_id, new_master_node_id,
            "New master should be different from old master in iteration {iteration}",
        );

        master = new_master;
        replicas = new_replicas;
    }

    for replica in replicas.into_iter().flatten() {
        replica.shutdown().await.unwrap();
    }
    master.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_state_replication() {
    let (Some(master), replicas, _tempdir, admin) = create_test_rollups(4).await else {
        return;
    };

    let (master, state) = setup_test_rollup_with_initial_state(master, &admin).await;

    let actions = vec![TestingAction::AcceptTx, TestingAction::QuerySetValue];
    let (master, replicas, mut state) =
        test_actions_against_replicas(&admin, (master, replicas, state), actions).await;

    let replicas = restart_replica(&admin, replicas, &mut state, 0).await;

    let actions = vec![
        TestingAction::NewDaSlot,
        TestingAction::Sleep { duration_ms: 100 },
    ];
    let (master, replicas, mut state) =
        test_actions_against_replicas(&admin, (master, replicas, state), actions).await;

    let replicas = restart_replica(&admin, replicas, &mut state, 0).await;

    let actions = vec![
        TestingAction::AcceptTxs { count: 10 },
        TestingAction::NewDaSlot,
        TestingAction::TryAcceptBadTx {
            invalid_reason: InvalidGeneration::DuplicateTransaction,
        },
        TestingAction::Sleep { duration_ms: 100 },
        TestingAction::NewDaSlot,
        TestingAction::Sleep { duration_ms: 100 },
        TestingAction::NewDaSlot,
        TestingAction::Sleep { duration_ms: 100 },
        TestingAction::NewDaSlot,
        TestingAction::Sleep { duration_ms: 1000 },
    ];
    let (master, replicas, mut state) =
        test_actions_against_replicas(&admin, (master, replicas, state), actions).await;

    let replicas = restart_replica(&admin, replicas, &mut state, 1).await;
    let replicas = restart_replica(&admin, replicas, &mut state, 2).await;

    let (master, replicas, state) = test_actions_against_replicas(
        &admin,
        (master, replicas, state),
        vec![TestingAction::QuerySetValue],
    )
    .await;

    // Silence unused variable warnings to keep the test easier to edit
    drop(state);

    // Shutdown replicas first
    for replica in replicas {
        replica.unwrap().shutdown().await.unwrap();
    }
    // Shut down master last, otherwise the postgres subscription will drop and replicas will error
    master.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_replica_transaction_rejection() {
    let (Some(mut master), mut replicas, _tempdir, admin) = create_test_rollups(2).await else {
        return;
    };

    // Setup initial state
    let (master_with_state, mut state) = setup_test_rollup_with_initial_state(master, &admin).await;
    master = master_with_state;

    let replica = replicas[0].take().unwrap();

    // Master should be able to accept transactions
    let master = run_action_against_test_rollup(
        master,
        &admin.private_key,
        TestingAction::AcceptTx,
        &mut state,
    )
    .await
    .unwrap();

    // Replica should reject transactions with replica mode error
    let replica = run_action_against_test_rollup(
        replica,
        &admin.private_key,
        TestingAction::ExpectFailTx {
            fail_reason: FailureReason::ReplicaMode,
        },
        &mut state,
    )
    .await
    .unwrap();

    // Shutdown master to trigger failover
    let master_builder = master.shutdown().await.unwrap();

    // Wait for failover
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Former replica should now be master and accept transactions
    let new_master = run_action_against_test_rollup(
        replica,
        &admin.private_key,
        TestingAction::AcceptTx,
        &mut state,
    )
    .await
    .unwrap();

    // Restart old master - it should now be a replica
    let old_master_as_replica = master_builder.start().await.unwrap();

    // Give time for the restarted node to sync and become a replica
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Old master (now replica) should reject transactions with replica mode error
    let old_master_as_replica = run_action_against_test_rollup(
        old_master_as_replica,
        &admin.private_key,
        TestingAction::ExpectFailTx {
            fail_reason: FailureReason::ReplicaMode,
        },
        &mut state,
    )
    .await
    .unwrap();

    // Cleanup
    old_master_as_replica.shutdown().await.unwrap();
    new_master.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_state_replication_with_failover() {
    let (Some(mut master), replicas, _tempdir, admin) = create_test_rollups(3).await else {
        return;
    };

    // Setup initial state
    let (master_with_state, state) = setup_test_rollup_with_initial_state(master, &admin).await;
    master = master_with_state;

    // Accept some transactions with master running
    let actions = vec![TestingAction::AcceptTx, TestingAction::AcceptTx];
    let (master, replicas, state) =
        test_actions_against_replicas(&admin, (master, replicas, state), actions).await;

    // Shutdown master to trigger failover
    let master_builder = master.shutdown().await.unwrap();

    // Wait for failover to occur
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Identify new master and replicas
    let all_remaining: Vec<TestRollup<TestBlueprint>> = replicas.into_iter().flatten().collect();
    let mut all_remaining = all_remaining.into_iter();
    let (new_master, new_replicas) = identify_master_and_replicas(
        all_remaining.next().unwrap(),
        all_remaining.map(Some).collect(),
    )
    .await
    .unwrap();

    // Accept transactions while old master is down
    let actions = vec![TestingAction::AcceptTx, TestingAction::AcceptTx];
    let (new_master, new_replicas, mut state) =
        test_actions_against_replicas(&admin, (new_master, new_replicas, state), actions).await;

    // Restart old master - it should become a replica and sync state
    let old_master_as_replica = master_builder.start().await.unwrap();

    // Give time for old master to sync up as replica
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify old master (now replica) has synced the state from while it was down
    let old_master_as_replica = run_action_against_test_rollup(
        old_master_as_replica,
        &admin.private_key,
        TestingAction::QuerySetValue,
        &mut state,
    )
    .await
    .unwrap();

    // Continue with more transactions to ensure everything works
    let actions = vec![TestingAction::AcceptTx, TestingAction::NewDaSlot];
    let mut all_nodes = vec![Some(new_master), Some(old_master_as_replica)];
    all_nodes.extend(new_replicas);
    let (new_master, remaining_replicas, state) = test_actions_against_replicas(
        &admin,
        (
            all_nodes[0].take().unwrap(),
            all_nodes.into_iter().skip(1).collect(),
            state,
        ),
        actions,
    )
    .await;

    // Final check that all nodes have consistent state
    let (final_master, final_replicas, final_state) = test_actions_against_replicas(
        &admin,
        (new_master, remaining_replicas, state),
        vec![TestingAction::QuerySetValue],
    )
    .await;

    // Cleanup
    for replica in final_replicas.into_iter().flatten() {
        replica.shutdown().await.unwrap();
    }
    final_master.shutdown().await.unwrap();

    // Silence unused variable warning
    drop(final_state);
}
