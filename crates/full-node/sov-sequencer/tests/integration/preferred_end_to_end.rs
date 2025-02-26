//! Integration tests for the preferred sequencer that use [`RollupBuilder`] and
//! thus test sequencer + node interactions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use sov_api_spec::types::{self as api_types, PublishBatchBody, TxReceiptResult};
use sov_mock_da::storable::layer::StorableMockDaLayer;
use sov_mock_da::BlockProducingConfig;
use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
use sov_modules_api::prelude::*;
use sov_modules_api::{DispatchCall, RawTx, Runtime};
use sov_modules_stf_blueprint::GenesisParams;
use sov_node_client::NodeClient;
use sov_paymaster::{Paymaster, PaymasterConfig};
use sov_rest_utils::ResponseObject;
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::node::ledger_api::IncludeChildren;
use sov_stf_runner::processes::RollupProverConfig;
use sov_test_modules::hooks_count::HooksCount;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, TestRollup};
use sov_test_utils::{
    default_test_signed_transaction, generate_optimistic_runtime_with_kernel, RtAgnosticBlueprint,
    TestSpec,
};
use sov_value_setter::{ValueSetter, ValueSetterConfig};
use test_strategy::Arbitrary;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tracing::{debug, info};

use crate::utils::{generate_paymaster_tx, generate_txs, ModuleWithVersionedStateAccessInSlotHook};

generate_optimistic_runtime_with_kernel!(
    TestRuntime <=
    kernel_type: sov_kernels::soft_confirmations::SoftConfirmationsKernel<'a, S>,
    value_setter: ValueSetter<S>,
    hooks_count: HooksCount<S>,
    paymaster: Paymaster<S>,
    slot_hook_checker: ModuleWithVersionedStateAccessInSlotHook<S>
);

type TestBlueprint = RtAgnosticBlueprint<TestSpec, TestRuntime<TestSpec>>;

/// All the interesting "things" that can happen during sequencer operations, and to
/// which the sequencer ought to know how to respond.
#[derive(Debug, Clone, Arbitrary)]
enum TestingAction {
    /// Never generated automatically because tests would slow down wayyy too
    /// much. Useful for debugging.
    #[weight(0)]
    Sleep { duration_ms: u64 },
    /// The node is immediately shutdown and restarted, to catch possible losses
    /// of soft-confirmed transactions and state initialization bugs.
    Restart,
    /// A client submits a valid transaction to be included in the next batch,
    /// and for which a soft confirmation ought to be provided immediately.
    #[weight(5)] // Make it more likely to be picked (this is where all juicy stuff happens)
    AcceptTx,
    /// Shorthand for a bunch of transactions in quick succession.
    AcceptTxs {
        #[strategy(0..10usize)]
        count: usize,
    },
    /// A client submits an **invalid** transactions, asking for it to be
    /// included in the next batch (it won't, as it's invalid).
    #[weight(2)]
    TryAcceptBadTx { invalid_reason: InvalidGeneration },
    /// A client queries the nonce for a given address.
    ///
    /// This is an easy and effective way for us to check that all pending
    /// transactions have actually been processed by the sequencer, and that
    /// its state changes are visible to REST API clients.
    ///
    /// TODO(@neysofu): Switch to the message generator.
    #[weight(8)]
    QuerySetValue,
    /// Like [`TestingAction::QuerySetValue`], but historical queries.
    ///
    /// FIXME(@neysofu): historical queries only work for node-processed slots,
    /// and not soft confirmations. This is arguably fine, but this test feature
    /// doesn't take that into account and is currently broken.
    #[weight(0)]
    #[allow(dead_code)]
    QuerySetValueHistorical,
    /// A new DA slot will be produced and made available to the node and sequencer.
    NewDaSlot,
    /// Terminates the in-progress batch and publishes it to the DA layer.
    PublishBatch,
}

/// An invalid nonce.
#[derive(Debug, Clone, Arbitrary)]
enum InvalidGeneration {
    DuplicateTransaction,
    TooOld,
}

async fn new_test_rollup(
    dir: Arc<tempfile::TempDir>,
    genesis_params: GenesisParams<<TestRuntime<TestSpec> as Runtime<TestSpec>>::GenesisConfig>,
    minimum_profit_per_tx: u128,
) -> TestRollup<TestBlueprint> {
    const FINALIZATION_BLOCKS: u32 = 3;
    let sequencer_addr = genesis_params.runtime.sequencer_registry.seq_da_address;

    RollupBuilder::<TestBlueprint>::new(
        GenesisSource::CustomParams(genesis_params),
        BlockProducingConfig::OnAnySubmit {
            block_wait_timeout_ms: None,
        },
        FINALIZATION_BLOCKS,
    )
    .with_preferred_seq_min_profit_per_tx(minimum_profit_per_tx)
    .set_config(|c| {
        c.rollup_prover_config = Some(RollupProverConfig::Skip);
        c.automatic_batch_production = false;
        c.storage = dir;
    })
    .set_da_config(|c| c.sender_address = sequencer_addr)
    .start()
    .await
    .unwrap()
}

#[derive(Debug, Default)]
struct TestState {
    value_by_slot_number: HashMap<SlotNumber, u64>,
    _current_slot_number: SlotNumber,
    next_generation: u64,
    current_value: u64,
}

// FIXME(@neysofu): this test is not broken due to correctness bugs in the
// sequencer, but rather because generated testing scenarios sometimes are
// oversized and the node can't keep up with the sequencer. TODO: find a solution.
//#[test]
//fn random_edge_cases_and_complex_scenarios() {
//    use proptest::prelude::*;
//    use proptest::test_runner::{Config, TestRunner};
//
//    let mut runner = TestRunner::new(Config::with_cases(1));
//    let result = runner.run(
//        &proptest::collection::vec(any::<TestingAction>(), 0..50),
//        |actions| {
//            let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
//                .enable_all()
//                .build()
//                .unwrap();
//            tokio_runtime.block_on(async {
//                preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
//            });
//
//            Ok(())
//        },
//    );
//
//    result.unwrap();
//}

#[tokio::test(flavor = "multi_thread")]
async fn txs_below_min_fee_are_rejected() {
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let admin = genesis_config.additional_accounts[0].clone();

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

    let dir = Arc::new(tempfile::tempdir().unwrap());

    let test_rollup = new_test_rollup(dir.clone(), genesis_params, 1).await;

    // Produce a few blocks to DA blocks to make sure there's a finalized slot after genesis.
    test_rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();
    sleep(Duration::from_millis(200)).await;

    let client = test_rollup.api_client.clone();
    let tx = tx_set_value(&admin.private_key, 0, 7);
    let Err(e) = client
        .accept_tx(&api_types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&tx),
        })
        .await
    else {
        panic!("Tx must have been rejected for insufficient fee");
    };
    let err_message = e.to_string();
    assert!(
        err_message.contains("This transaction did not pay a sufficient net fee."),
        "Full error message does not contain expect part: {}",
        err_message
    );
}

/// Ensure that we use the correct visible slot number when replaying transactions after a call to `update_state` in the sequencer.
/// The key thing that this test does is to execute the same transaction 3 times - once in the sequencer via `accept_tx`, once via `update_state`
/// and once in the node. Everything else is implementation details.
///
/// # How it works (currently)
/// Here's how the test works currently - feel free to change this as the sequencer logic evolves.
///  1. Produce enough empty *DA blocks* that the sequencer will produce an empty batch
///  2. Before including that first empty batch on DA, submit a transaction to the sequencer which asserts the correct visible slot number.
///      This will cause the sequencer to start bulding a new batch on top of the updated state.
///  3. Include the empty batch on DA, and wait for the node to process it. This triggers a call to `update_state` in the sequencer, which will panic on error.
///  4. Accept the sequencer's new batch (which contains the transaction asserting the correct visible slot number) onto the DA layer. Defensively assert that it
///     gets processed correctly by the node.
#[tokio::test(flavor = "multi_thread")]
async fn replay_uses_correct_visible_slot_number() {
    const FINALIZATION_BLOCKS: u32 = 0;
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let admin = genesis_config.additional_accounts[0].clone();

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

    let dir = Arc::new(tempfile::tempdir().unwrap());

    let da_layer = Arc::new(tokio::sync::RwLock::new(
        StorableMockDaLayer::new_in_memory(FINALIZATION_BLOCKS)
            .await
            .unwrap(),
    ));
    let test_rollup = {
        let sequencer_addr = genesis_params.runtime.sequencer_registry.seq_da_address;
        RollupBuilder::<TestBlueprint>::new(
            GenesisSource::CustomParams(genesis_params),
            BlockProducingConfig::Manual, // Use manual block production to be sure that the changes are happening in the sequencer only, not the node.
            FINALIZATION_BLOCKS,
        )
        .set_config(|c| {
            c.rollup_prover_config = Some(RollupProverConfig::Skip);
            c.storage = dir;
        })
        .set_da_config(|c| {
            c.sender_address = sequencer_addr;
            c.da_layer = Some(da_layer.clone());
        })
        .start()
        .await
        .unwrap()
    };
    let mut slot_subscription = test_rollup
        .api_client
        .subscribe_slots_with_children(IncludeChildren::new(true))
        .await
        .unwrap();
    // First, produce two empty blocks. After the second one, the preferred sequencer will produce an empty batch in an attempt
    // to keep the visible_slot_number within 2 of the DA slot number.
    da_layer.write().await.produce_block().await.unwrap();
    slot_subscription.next().await.unwrap().unwrap();
    da_layer.write().await.produce_block().await.unwrap();
    slot_subscription.next().await.unwrap().unwrap();
    // Wait for the node to process the empty blocks. This ensures that the sequencer has time to produce an empty batch.
    // Right here (invisibly) the preferred sequencer will produce its empty batch and send it to DA
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Produce a block with a transaction that asserts the correct visible slot number.
    // Note: The exact number height asserted here is not important, as long as it's correct
    // at the time we submit the transaction - if we change the sequencer logic, this number may need to be updated.
    let tx = tx_assert_visible_slot_number(&admin.private_key, 0, 2);
    test_rollup
        .api_client
        .accept_tx(&api_types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&tx),
        })
        .await
        .unwrap();

    // Produce a block. This places the (invisibly produced) empty preferred batch on DA, triggering the sequencer to replay the soft-confirmed transaction
    // on top of that new state. The replay will fail if the visible slot number was not correctly set.
    da_layer.write().await.produce_block().await.unwrap();
    let slot = slot_subscription.next().await.unwrap().unwrap();
    // Right here, we check that we really did receive an empty batch from the preferred sequencer.
    // This is a test of the test logic, not a test of the sequencer - it's perfectly valid to modify
    // the sequencer such that a batch is not produced here - but in that case we need to update this test.
    assert_eq!(slot.number, 3);
    assert_eq!(slot.batches.len(), 1);
    assert_eq!(slot.batches[0].txs.len(), 0);

    // Produce another block. This one will be empty because the sequencer is currently very conservative about
    // waiting to have some finalized blocks available before producing a batch. If we update the sequencer to be as eager
    // as safely possible about producing batches, we will need to remove this call to `produce_block`.
    da_layer.write().await.produce_block().await.unwrap();
    slot_subscription.next().await.unwrap().unwrap();

    // Ensure that the sequencer has time to see the updated state and submit its batch containing the transaction to DA.
    tokio::time::sleep(Duration::from_millis(100)).await;
    da_layer.write().await.produce_block().await.unwrap();
    let next = slot_subscription.next().await.unwrap().unwrap();

    assert_eq!(next.number, 5);
    assert_eq!(
        next.batches[0].txs[0].receipt.result,
        TxReceiptResult::Successful,
        "Replay in the sequencer was successful, but replay on the node failed: {:?}",
        next.batches[0],
    );
}

/// This test checks that the visible hash of the rollup block is the same in the node and the sequencer.
///
/// This test currently works by running a loop of...
/// - Send a transaction to trigger the sequencer to start a batch.
/// - Query the current state root from the sequencer.
/// - Send a transaction that asserts the correct state root. Ensure it is accepted
/// - Produce a block, triggering the sequencer to close out its current batch and post it on DA
/// - Check that the state root assertion suceeded on the node as well.
#[tokio::test(flavor = "multi_thread")]
async fn visible_hashes_match_across_node_and_sequencer() {
    const FINALIZATION_BLOCKS: u32 = 0;
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let admin = genesis_config.additional_accounts[0].clone();

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

    let dir = Arc::new(tempfile::tempdir().unwrap());

    let da_layer = Arc::new(tokio::sync::RwLock::new(
        StorableMockDaLayer::new_in_memory(FINALIZATION_BLOCKS)
            .await
            .unwrap(),
    ));
    let test_rollup = {
        let sequencer_addr = genesis_params.runtime.sequencer_registry.seq_da_address;
        RollupBuilder::<TestBlueprint>::new(
            GenesisSource::CustomParams(genesis_params),
            BlockProducingConfig::Manual, // Use manual block production to be sure that the changes are happening in the sequencer only, not the node.
            FINALIZATION_BLOCKS,
        )
        .set_config(|c| {
            c.rollup_prover_config = Some(RollupProverConfig::Skip);
            c.storage = dir;
        })
        .set_da_config(|c| {
            c.sender_address = sequencer_addr;
            c.da_layer = Some(da_layer.clone());
        })
        .start()
        .await
        .unwrap()
    };

    let mut slot_subscription = test_rollup
        .api_client
        .subscribe_slots_with_children(IncludeChildren::new(true))
        .await
        .unwrap();

    #[derive(Debug, serde::Deserialize)]
    struct StateRootResponse {
        root_hashes: Vec<u8>,
    }
    #[derive(Debug, serde::Deserialize)]
    struct ValueResponse {
        value: StateRootResponse,
    }
    async fn get_state_root(test_rollup: &TestRollup<TestBlueprint>) -> StateRootResponse {
        let state_root_url = format!(
            "{}/modules/hooks-count/state/latest-state-root/",
            test_rollup.api_client.baseurl()
        );
        let response = test_rollup
            .api_client
            .client()
            .get(state_root_url)
            .send()
            .await
            .unwrap();
        let response = response
            .json::<ResponseObject<ValueResponse>>()
            .await
            .expect("Hooks must have run");
        let root = response
            .data
            .ok_or_else(|| anyhow::anyhow!("No state root in response"))
            .unwrap();
        root.value
    }
    // Produce some empty blocks to ensure that the sequencer has a batch in progress.
    da_layer.write().await.produce_block().await.unwrap();
    slot_subscription.next().await.unwrap().unwrap();
    da_layer.write().await.produce_block().await.unwrap();
    slot_subscription.next().await.unwrap().unwrap();
    sleep(Duration::from_millis(50)).await;

    // Run a few rounds of checking the state root to be extra sure nothing gets screwed up over time.
    let mut current_nonce = 0;
    for i in 0..10 {
        // Send a transaction to ensure that the sequencer has a batch in progress. This is necessary
        // because we start a new rollup block (with a new visible hash) each time we start a batch.
        let tx = tx_set_value(&admin.private_key, current_nonce, i).clone();
        test_rollup
            .api_client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();
        current_nonce += 1;

        // Query the current state root from the node.
        let root = get_state_root(&test_rollup).await;
        tracing::info!(
            "Sending assert state root tx: {}",
            hex::encode(&root.root_hashes)
        );

        // Send a transaction that asserts the correct state root.
        let tx = tx_assert_state_root(&admin.private_key, current_nonce, root.root_hashes.clone())
            .clone();
        test_rollup
            .api_client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();

        current_nonce += 1;

        // Produce a block. This will trigger the sequencer to close out its current batch and start a new one.
        da_layer.write().await.produce_block().await.unwrap();
        let slot = slot_subscription.next().await.unwrap().unwrap();
        if !slot.batches.is_empty() && !slot.batches[0].txs.is_empty() {
            // Assert that the second transaction in the batch (the one that asserts the state root) succeeded.
            assert_eq!(
                slot.batches[0].txs[1].receipt.result,
                TxReceiptResult::Successful
            );
        }
        // Sleep to ensure that the sequencer has time to process `update_state` and submit its batch before the next loop iteration.
        sleep(Duration::from_millis(200)).await;
    }
}

/// This test checks that state changes from the begin/end slot and finalize hooks are visible via the sequencer's REST API.
///
/// It works by producing several batches in the sequencer (causing the hooks to be run) without every publishing those batches
/// to DA (ensuring that the state changes are not visible to the node), then querying the state via the REST API.
#[tokio::test(flavor = "multi_thread")]
async fn test_hooks_state_is_visible() {
    const FINALIZATION_BLOCKS: u32 = 3;
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let admin = genesis_config.additional_accounts[0].clone();

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

    let dir = Arc::new(tempfile::tempdir().unwrap());

    let da_layer = Arc::new(tokio::sync::RwLock::new(
        StorableMockDaLayer::new_in_memory(FINALIZATION_BLOCKS)
            .await
            .unwrap(),
    ));
    let test_rollup = {
        let sequencer_addr = genesis_params.runtime.sequencer_registry.seq_da_address;
        RollupBuilder::<TestBlueprint>::new(
            GenesisSource::CustomParams(genesis_params),
            BlockProducingConfig::Manual, // Use manual block production to be sure that the changes are happening in the sequencer only, not the node.
            FINALIZATION_BLOCKS,
        )
        .set_config(|c| {
            c.automatic_batch_production = false;
            c.rollup_prover_config = Some(RollupProverConfig::Skip);
            c.storage = dir;
        })
        .set_da_config(|c| {
            c.sender_address = sequencer_addr;
            c.da_layer = Some(da_layer.clone());
        })
        .start()
        .await
        .unwrap()
    };

    test_rollup
        .da_service
        .produce_n_blocks_now(8)
        .await
        .unwrap();
    sleep(Duration::from_millis(200)).await;

    // By the time we query here, the sequencer has *started* the next slot, so it has run the begin slot hook a second time.
    let client = NodeClient::new(test_rollup.api_client.baseurl())
        .await
        .unwrap();

    let query_hook_counter = |hook_name: &'static str| async {
        #[derive(Debug, serde::Deserialize)]
        struct ValueResponse {
            value: u32,
        }
        let hook_name = hook_name.to_string();
        client
            .query_rest_endpoint::<ResponseObject<ValueResponse>>(&format!(
                "/modules/hooks-count/state/{}-hook-count",
                hook_name
            ))
            .await
            .unwrap()
            .data
            .unwrap()
            .value
    };

    let begin_slot_count = query_hook_counter("begin-rollup-block").await;
    assert_eq!(begin_slot_count, 0);
    let begin_slot_count = query_hook_counter("end-rollup-block").await;
    assert_eq!(begin_slot_count, 0);

    {
        let txs = generate_txs(admin.private_key.clone()).clone();
        for tx in txs {
            client
                .client
                .accept_tx(&api_types::AcceptTxBody {
                    body: BASE64_STANDARD.encode(&tx.raw_tx),
                })
                .await
                .unwrap();
        }
    }

    let begin_slot_count = query_hook_counter("begin-rollup-block").await;
    assert_eq!(begin_slot_count, 1);

    //  since we haven't finished building this batch, the end slot hook hasn't been run - so its value is still 0
    let end_slot_count = query_hook_counter("end-rollup-block").await;
    assert_eq!(end_slot_count, 0);
    // was run once during genesis
    let finalize_count = query_hook_counter("finalize").await;
    assert_eq!(finalize_count, 1);

    test_rollup
        .da_service
        .produce_n_blocks_now(10)
        .await
        .unwrap();
    sleep(Duration::from_millis(200)).await;

    // Finish the in-progress batch.
    test_rollup
        .api_client
        .publish_batch(&PublishBatchBody {
            transactions: vec![],
        })
        .await
        .unwrap();

    let begin_slot_count = query_hook_counter("begin-rollup-block").await;
    assert_eq!(begin_slot_count, 1);
    let end_slot_count = query_hook_counter("end-rollup-block").await;
    assert_eq!(end_slot_count, 1);
    let finalize_count = query_hook_counter("finalize").await;
    assert_eq!(finalize_count, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_production_and_accept_tx() {
    let mut actions = vec![];
    for i in 1..20 {
        actions.push(TestingAction::AcceptTxs { count: i });
        actions.push(TestingAction::AcceptTx);
    }

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
}

// Checks that transactions that are not sequencer safe are rejected
// when the sender address is not configured as an admin in the sequencer config.
#[tokio::test(flavor = "multi_thread")]
async fn not_sequencer_safe_txs_are_restricted() {
    let mut genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    genesis_config.initial_sequencer.bond *= 100;

    let admin = genesis_config.additional_accounts[0].clone();

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

    let dir = Arc::new(tempfile::tempdir().unwrap());

    let test_rollup = new_test_rollup(dir.clone(), genesis_params, 0).await;

    test_rollup
        .da_service
        .produce_n_blocks_now(10)
        .await
        .unwrap();

    // Wait for all blocks to be processed by the node+sequencer. TODO: better
    // logic not prone to race conditions.
    sleep(Duration::from_millis(500)).await;

    let tx = generate_paymaster_tx::<TestRuntime<TestSpec>>(admin.private_key.clone());
    {
        if let Err(e) = test_rollup
            .client
            .client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
        {
            assert!(
                e.to_string().contains("Only designated admins are allowed"),
                "Unexpected error: {}",
                e
            );
        } else {
            panic!("Sequencer accepted admin tx from non-admin sender");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_and_query_value() {
    let actions = vec![TestingAction::Restart, TestingAction::QuerySetValue];

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_production_and_state_query() {
    let actions = vec![
        TestingAction::AcceptTx,
        TestingAction::AcceptTxs { count: 4 },
        TestingAction::AcceptTxs { count: 4 },
        TestingAction::AcceptTxs { count: 4 },
        TestingAction::AcceptTxs { count: 4 },
        TestingAction::AcceptTxs { count: 4 },
        TestingAction::AcceptTxs { count: 4 },
        TestingAction::QuerySetValue,
        TestingAction::AcceptTxs { count: 4 },
        TestingAction::AcceptTx,
        TestingAction::QuerySetValue,
    ];

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn api_state_race_condition_regression() {
    let actions = vec![
        TestingAction::QuerySetValue,
        TestingAction::AcceptTx,
        TestingAction::QuerySetValue,
        TestingAction::AcceptTxs { count: 1 },
        TestingAction::QuerySetValue,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::TryAcceptBadTx {
            invalid_reason: InvalidGeneration::DuplicateTransaction,
        },
        TestingAction::TryAcceptBadTx {
            invalid_reason: InvalidGeneration::TooOld,
        },
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::QuerySetValue,
        TestingAction::TryAcceptBadTx {
            invalid_reason: InvalidGeneration::TooOld,
        },
        TestingAction::NewDaSlot {},
        TestingAction::QuerySetValue,
        TestingAction::QuerySetValue,
        TestingAction::QuerySetValue,
        TestingAction::QuerySetValue,
        TestingAction::AcceptTxs { count: 4 },
        TestingAction::TryAcceptBadTx {
            invalid_reason: InvalidGeneration::TooOld,
        },
        TestingAction::AcceptTxs { count: 4 },
        TestingAction::QuerySetValue,
    ];

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_after_big_batch_regression() {
    let actions = vec![
        TestingAction::AcceptTxs { count: 1 },
        TestingAction::AcceptTxs { count: 5 },
        TestingAction::AcceptTx,
        TestingAction::Restart,
        TestingAction::AcceptTxs { count: 10 },
        TestingAction::Restart,
        TestingAction::AcceptTx,
    ];

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn flaky_batch_production_with_immediate_finalization() {
    let actions = vec![
        TestingAction::AcceptTxs { count: 1 },
        TestingAction::AcceptTxs { count: 50 },
        TestingAction::Restart,
        TestingAction::AcceptTx,
        TestingAction::Sleep { duration_ms: 50 },
        TestingAction::Restart,
        TestingAction::AcceptTxs { count: 50 },
        TestingAction::Restart,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::AcceptTxs { count: 3 },
        TestingAction::AcceptTxs { count: 50 },
    ];

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
}

async fn preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions: Vec<TestingAction>) {
    let mut genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    genesis_config.initial_sequencer.bond *= 100;

    let admin = genesis_config.additional_accounts[0].clone();

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

    let dir = Arc::new(tempfile::tempdir().unwrap());

    let test_rollup = new_test_rollup(dir.clone(), genesis_params, 0).await;

    test_rollup
        .da_service
        .produce_n_blocks_now(10)
        .await
        .unwrap();

    // Wait for all blocks to be processed by the node+sequencer. TODO: better
    // logic not prone to race conditions.
    sleep(Duration::from_millis(500)).await;

    let client = test_rollup.api_client.clone();

    let mut test_state = TestState {
        next_generation: 10, // initialize to a higher generation so that "invalid generation" actions are always possible
        ..Default::default()
    };

    {
        let txs = generate_txs(admin.private_key.clone()).clone();
        for tx in txs {
            client
                .accept_tx(&api_types::AcceptTxBody {
                    body: BASE64_STANDARD.encode(&tx.raw_tx),
                })
                .await
                .unwrap();
            test_state.next_generation += 1;
        }
    }

    // initialize nonce value
    {
        let tx = tx_set_value(
            &admin.private_key,
            test_state.next_generation,
            test_state.current_value,
        );
        client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();
        test_state.next_generation += 1;
    }

    let mut test_rollup = Some(test_rollup);

    for (i, action) in actions.iter().enumerate() {
        let new_test_rollup_res = run_action_against_test_rollup(
            test_rollup.take().unwrap(),
            rt_genesis_config.clone(),
            &admin.private_key,
            action.clone(),
            &mut test_state,
        )
        .await;

        match new_test_rollup_res {
            Ok(new_test_rollup) => test_rollup = Some(new_test_rollup),
            Err(e) => {
                println!("Action history: {:#?}", actions[..=i].to_vec());
                println!("test state: {:#?}", test_state);
                panic!("Error: {:#?}", e);
            }
        }
    }

    test_rollup.take().unwrap().shutdown().await.unwrap();
}

async fn run_action_against_test_rollup(
    test_rollup: TestRollup<TestBlueprint>,
    rt_genesis_params: <TestRuntime<TestSpec> as Runtime<TestSpec>>::GenesisConfig,
    key: &Ed25519PrivateKey,
    action: TestingAction,
    test_state: &mut TestState,
) -> anyhow::Result<TestRollup<TestBlueprint>> {
    assert!(test_state.next_generation > 0);

    info!(
        ?action,
        test_state.next_generation, "Executing testing action"
    );

    match action {
        TestingAction::Sleep { duration_ms } => {
            sleep(Duration::from_millis(duration_ms)).await;
        }
        TestingAction::Restart => {
            let storage_dir = test_rollup.storage.clone();
            let genesis_params = GenesisParams {
                runtime: rt_genesis_params,
            };

            test_rollup.shutdown().await?;

            return Ok(new_test_rollup(storage_dir, genesis_params, 0).await);
        }
        TestingAction::TryAcceptBadTx { invalid_reason } => {
            let tx = match invalid_reason {
                InvalidGeneration::DuplicateTransaction => tx_set_value(
                    key,
                    test_state.next_generation - 1,
                    test_state.current_value,
                ),
                InvalidGeneration::TooOld => {
                    let bad_generation = test_state.next_generation
                        - 1
                        - config_value!("PAST_TRANSACTION_GENERATIONS");
                    println!("Generating generation {bad_generation} for reason::TooOld");
                    tx_set_value(key, bad_generation, test_state.current_value + 1)
                }
            };

            anyhow::ensure!(test_rollup
                .api_client
                .accept_tx(&api_types::AcceptTxBody {
                    body: BASE64_STANDARD.encode(&tx),
                })
                .await
                .is_err());
        }
        TestingAction::AcceptTx => {
            let tx = tx_set_value(
                key,
                test_state.next_generation,
                test_state.current_value + 1,
            );

            test_state.next_generation += 1;
            test_state.current_value += 1;

            test_rollup
                .api_client
                .accept_tx(&api_types::AcceptTxBody {
                    body: BASE64_STANDARD.encode(&tx),
                })
                .await?;
        }
        TestingAction::AcceptTxs { count } => {
            for _ in 0..count {
                let tx = tx_set_value(
                    key,
                    test_state.next_generation,
                    test_state.current_value + 1,
                );

                test_state.next_generation += 1;
                test_state.current_value += 1;

                test_rollup
                    .api_client
                    .accept_tx(&api_types::AcceptTxBody {
                        body: BASE64_STANDARD.encode(&tx),
                    })
                    .await?;
            }
        }
        TestingAction::PublishBatch => {
            test_rollup
                .api_client
                .publish_batch(&PublishBatchBody {
                    transactions: vec![],
                })
                .await
                .ok();
        }
        TestingAction::NewDaSlot { .. } => {}
        TestingAction::QuerySetValueHistorical => {
            for (slot_number, value) in test_state.value_by_slot_number.iter() {
                info!(
                    %slot_number,
                    %value,
                    "Historical query of value",
                );
                query_set_value(&test_rollup, Some(slot_number.get()), *value).await?;
            }
        }
        TestingAction::QuerySetValue => {
            query_set_value(&test_rollup, None, test_state.current_value).await?;
        }
    }

    Ok(test_rollup)
}

async fn query_set_value(
    test_rollup: &TestRollup<TestBlueprint>,
    slot_number: Option<u64>,
    expected: u64,
) -> anyhow::Result<()> {
    let url = format!(
        "/modules/value-setter/state/value{}",
        if let Some(slot_number) = slot_number {
            format!("?rollup_height={}", slot_number)
        } else {
            "".to_string()
        }
    );

    let response = test_rollup
        .client
        .query_rest_endpoint::<ResponseObject<serde_json::Value>>(&url)
        .await?;

    debug!(?response, "Querying value");

    let found_value = response.data.unwrap()["value"].as_u64().unwrap();

    anyhow::ensure!(found_value == expected);

    Ok(())
}

fn tx_set_value(key: &Ed25519PrivateKey, nonce: u64, value_to_set: u64) -> RawTx {
    let msg = <TestRuntime<TestSpec> as DispatchCall>::Decodable::ValueSetter(
        sov_value_setter::CallMessage::SetValue {
            value: value_to_set as u32,
            gas: None,
        },
    );

    encode_call(key, nonce, &msg)
}

fn tx_assert_visible_slot_number(
    key: &Ed25519PrivateKey,
    nonce: u64,
    assert_visible_slot_number: u64,
) -> RawTx {
    let msg = <TestRuntime<TestSpec> as DispatchCall>::Decodable::HooksCount(
        sov_test_modules::hooks_count::CallMessage::AssertVisibleSlotNumber {
            expected_visible_slot_number: assert_visible_slot_number,
        },
    );

    encode_call(key, nonce, &msg)
}

fn tx_assert_state_root(
    key: &Ed25519PrivateKey,
    nonce: u64,
    expected_state_root: Vec<u8>,
) -> RawTx {
    let msg = <TestRuntime<TestSpec> as DispatchCall>::Decodable::HooksCount(
        sov_test_modules::hooks_count::CallMessage::AssertStateRoot {
            expected_state_root,
        },
    );

    encode_call(key, nonce, &msg)
}

fn encode_call(
    key: &Ed25519PrivateKey,
    nonce: u64,
    call_message: &<TestRuntime<TestSpec> as DispatchCall>::Decodable,
) -> RawTx {
    let tx = default_test_signed_transaction::<TestRuntime<TestSpec>, TestSpec>(
        key,
        call_message,
        nonce,
        &<TestRuntime<TestSpec> as Runtime<TestSpec>>::CHAIN_HASH,
    );

    RawTx::new(borsh::to_vec(&tx).unwrap())
}

mod tests_with_basic_kernel {
    use sov_modules_stf_blueprint::GenesisParams;
    use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder};
    use sov_test_utils::{RtAgnosticBlueprint, TEST_DEFAULT_MOCK_DA_PERIODIC_PRODUCING};

    use super::{
        generate_optimistic_runtime_with_kernel, HighLevelOptimisticGenesisConfig, TestSpec,
    };
    generate_optimistic_runtime_with_kernel!(
        RtWithBasicKernel <=
        kernel_type: sov_kernels::basic::BasicKernel<'a, S>,
    );

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(
        expected = "Attempting to use preferred sequencer with an incompatible rollup. Set your sequencer config to `standard` in your rollup's config.toml file or change your kernel to be compatible with soft confirmations."
    )]
    async fn preferred_sequencer_panics_with_basic_kernel() {
        let genesis_config = HighLevelOptimisticGenesisConfig::generate();
        let genesis_params = GenesisParams {
            runtime: GenesisConfig::from_minimal_config(genesis_config.into()),
        };

        RollupBuilder::<RtAgnosticBlueprint<TestSpec, RtWithBasicKernel<TestSpec>>>::new(
            GenesisSource::CustomParams(genesis_params),
            TEST_DEFAULT_MOCK_DA_PERIODIC_PRODUCING,
            0,
        )
        .set_config(|conf| {
            conf.sequencer_config =
                sov_sequencer::SequencerKindConfig::Preferred(Default::default());
        })
        .start()
        .await
        .unwrap();
    }
}
