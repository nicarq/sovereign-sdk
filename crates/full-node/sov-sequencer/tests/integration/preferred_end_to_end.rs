//! Integration tests for the preferred sequencer that use [`RollupBuilder`] and
//! thus test sequencer + node interactions.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use borsh::{BorshDeserialize, BorshSerialize};
use futures::future;
use sov_api_spec::types::{self as api_types, TxReceiptResult};
use sov_mock_da::storable::layer::StorableMockDaLayer;
use sov_mock_da::BlockProducingConfig;
use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::prelude::*;
use sov_modules_api::{Amount, DispatchCall, Gas, GasArray, GasPrice, GasUnit, RawTx, Runtime};
use sov_modules_stf_blueprint::GenesisParams;
use sov_node_client::NodeClient;
use sov_paymaster::{Paymaster, PaymasterConfig};
use sov_rest_utils::ResponseObject;
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::node::ledger_api::IncludeChildren;
use sov_test_modules::hooks_count::HooksCount;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, RollupProverConfig, TestRollup};
use sov_test_utils::{
    default_test_signed_transaction, default_test_tx_details,
    generate_optimistic_runtime_with_kernel, test_signed_transaction, RtAgnosticBlueprint,
    TestSpec, TestUser, TEST_MAX_BATCH_SIZE,
};
use sov_value_setter::{ValueSetter, ValueSetterConfig};
use test_strategy::Arbitrary;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tracing::{debug, info};

use crate::utils::{
    generate_paymaster_tx, generate_txs, pause_update_state,
    ModuleWithVersionedStateAccessInSlotHook,
};

generate_optimistic_runtime_with_kernel!(
    TestRuntime <=
    kernel_type: sov_kernels::soft_confirmations::SoftConfirmationsKernel<'a, S>,
    value_setter: ValueSetter<S>,
    hooks_count: HooksCount<S>,
    paymaster: Paymaster<S>,
    slot_hook_checker: ModuleWithVersionedStateAccessInSlotHook<S>
);

// This allows for easily setting file sharing when using Docker Desktop.
fn tempdir_inside_codebase_dir() -> Arc<tempfile::TempDir> {
    Arc::new(tempfile::tempdir_in(std::env!("CARGO_TARGET_TMPDIR")).unwrap())
}

type TestBlueprint = RtAgnosticBlueprint<TestSpec, TestRuntime<TestSpec>>;

const DEFAULT_BLOCK_PRODUCING_CONFIG: BlockProducingConfig = BlockProducingConfig::OnBatchSubmit {
    block_wait_timeout_ms: None,
};

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
    /// Sets the pause state of update state execution to provided value.
    /// Allows disabling update_state from running inside the sequencer.
    PauseUpdateStateExecution(bool),
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
    automatic_batch_production: bool,
    max_batch_size_bytes: usize,
    block_producing_config: BlockProducingConfig,
    rollup_prover_config: Option<RollupProverConfig<MockZkvm>>,
) -> Option<TestRollup<TestBlueprint>> {
    const FINALIZATION_BLOCKS: u32 = 3;
    let sequencer_addr = genesis_params.runtime.sequencer_registry.seq_da_address;

    // We skip all docker (i.e. postgres) tests on our dev server due to firewall false positives
    // bricking the machine.
    // The dev machine has 96 threads, which we detect to disable postgres. Currently no dev or CI
    // setup uses a machine of exactly this size, though if this ever changes this will cause
    // false positives.
    const DEV_SERVER_CPUS: usize = 96;

    let mut builder_res = RollupBuilder::<TestBlueprint>::new(
        GenesisSource::CustomParams(genesis_params),
        block_producing_config,
        FINALIZATION_BLOCKS,
    )
    .set_config(|c| {
        c.rollup_prover_config = rollup_prover_config;
        c.automatic_batch_production = automatic_batch_production;
        c.storage = dir;
        c.max_batch_size_bytes = max_batch_size_bytes;
    })
    .set_da_config(|c| c.sender_address = sequencer_addr)
    .with_preferred_seq_min_profit_per_tx(minimum_profit_per_tx);

    if num_cpus::get() != DEV_SERVER_CPUS {
        builder_res = builder_res.with_postgres_sequencer().await.unwrap();
    } else {
        tracing::warn!("Running tests with postgres disabled in the sequencer! Detected machine with {DEV_SERVER_CPUS} threads, assuming we are running on the dev server.");
    }

    match Result::<_, anyhow::Error>::Ok(builder_res) {
        Ok(builder) => Some(builder.start().await.unwrap()),
        Err(e) => {
            if std::env::var("SOV_TEST_SKIP_DOCKER") == Ok("1".to_string()) {
                None
            } else {
                eprintln!("Error starting rollup builder: {:?}", e);
                eprintln!("To skip docker based tests run with the env var SOV_TEST_SKIP_DOCKER=1");
                panic!("Unable to proceed without docker");
            }
        }
    }
}

async fn create_test_rollup(
    minimum_profit_per_tx: u128,
    max_batch_size: usize,
) -> (Option<TestRollup<TestBlueprint>>, TestUser<TestSpec>) {
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

    let dir = tempdir_inside_codebase_dir();

    (
        new_test_rollup(
            dir.clone(),
            genesis_params,
            minimum_profit_per_tx,
            true,
            max_batch_size,
            BlockProducingConfig::Manual,
            None,
        )
        .await,
        admin,
    )
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
    let (test_rollup, admin) = create_test_rollup(1, TEST_MAX_BATCH_SIZE).await;

    let Some(test_rollup) = test_rollup else {
        return;
    };

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

#[tokio::test(flavor = "multi_thread")]
async fn seq_out_of_gas() {
    let mut genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let initial_gas_limit = GasUnit::from(config_value!("INITIAL_GAS_LIMIT"));
    let max_exec_gas_per_tx = GasUnit::from(config_value!("MAX_SEQUENCER_EXEC_GAS_PER_TX"));
    let price_array = config_value!("INITIAL_BASE_FEE_PER_GAS");
    let gas_price = GasPrice::<2>::from([
        Amount::from(price_array[0] as u64),
        Amount::from(price_array[1] as u64),
    ]);

    let max_amount_limit = initial_gas_limit.value(&gas_price);

    // Set very high initial balance for the admin.
    genesis_config.additional_accounts[0].available_gas_balance =
        max_amount_limit.checked_mul(Amount::new(10)).unwrap();

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

    let dir = tempdir_inside_codebase_dir();

    let Some(test_rollup) = new_test_rollup(
        dir.clone(),
        genesis_params,
        0,
        true,
        TEST_MAX_BATCH_SIZE,
        BlockProducingConfig::Manual,
        None,
    )
    .await
    else {
        // Docker issues, don't fail the test and just return early.
        return;
    };

    test_rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();

    sleep(Duration::from_millis(200)).await;

    let client = test_rollup.api_client.clone();
    test_rollup.pause_preferred_batches().await;

    // Produce the first transaction that nearly exhausts the gas slot limit.
    {
        let gas_to_charge = initial_gas_limit
            .checked_sub(&max_exec_gas_per_tx)
            .unwrap()
            .checked_sub(&GasUnit::from([200000, 200000]))
            .unwrap();

        let tx = tx_set_value_with_gas(
            &admin.private_key,
            0,
            7,
            Some(gas_to_charge),
            max_amount_limit,
        );
        client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();

        query_set_value(&test_rollup, None, 7).await.unwrap();
    }

    // The second transaction will be rejected because of the slot gas limit.
    {
        let tx = tx_set_value(&admin.private_key, 1, 8);

        let resp = client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap_err();

        let error_str = resp.to_string();
        assert!(error_str.contains("More transactions were submitted that the sequencer is allowed to put into a single batch."));
    }
    test_rollup.resume_preferred_batches().await;
    test_rollup
        .da_service
        .produce_n_blocks_now(2)
        .await
        .unwrap();
    sleep(Duration::from_millis(200)).await;

    // The third transaction is accepted because a new slot has started.
    {
        let tx = tx_set_value(&admin.private_key, 1, 9);

        client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();

        query_set_value(&test_rollup, None, 9).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn max_batch_size() {
    let max_batch_size = 1024;
    let (test_rollup, admin) = create_test_rollup(0, max_batch_size).await;

    let Some(test_rollup) = test_rollup else {
        return;
    };

    test_rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();
    sleep(Duration::from_millis(200)).await;

    let client = test_rollup.api_client.clone();

    // The transaction is rejected because it is too large.
    {
        let tx = tx_set_many_values(&admin.private_key, 0, vec![0; 1024]);

        let resp = client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap_err();

        let error_str = resp.to_string();
        assert!(error_str.contains("Transaction cannot be included in the batch"));
    }

    sleep(Duration::from_millis(200)).await;

    test_rollup.pause_preferred_batches().await;
    // The first and third transactions are processed, but the second and fourth are too large to be included in the batch.
    {
        let tx = tx_set_many_values(&admin.private_key, 0, vec![0; 128]);
        let _ = client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();

        let tx = tx_set_many_values(&admin.private_key, 1, vec![0; 1024]);
        let _ = client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap_err();

        let tx = tx_set_many_values(&admin.private_key, 1, vec![0; 512]);
        let _ = client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();

        let tx = tx_set_many_values(&admin.private_key, 2, vec![0; 512]);
        let _ = client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap_err();
    }

    test_rollup.resume_preferred_batches().await;
    test_rollup
        .da_service
        .produce_n_blocks_now(2)
        .await
        .unwrap();
    sleep(Duration::from_millis(200)).await;

    // Once we start creating a fresh batch, we can insert a transaction that was previously rejected.
    {
        let tx = tx_set_many_values(&admin.private_key, 2, vec![0; 512]);
        let _ = client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();
    }
}

/// Test that the sequencer can compute state roots for itself to avoid panics.
///
/// This test works by causing the node to fall far behind the sequencer in processsing rollup blocks.
/// This is done by preventing the seuqencer batches from being included on DA, while still producing lots of DA blocks.
///
/// Since there are always new finalized blocks available, the sequencer will happily create new rollup blocks far in advance of the node, triggering the case we care about.
#[tokio::test(flavor = "multi_thread")]
async fn flaky_test_state_root_computation_when_blobs_are_delayed() {
    std::env::set_var("SOV_TEST_CONST_OVERRIDE_STATE_ROOT_DELAY_BLOCKS", "1");
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

    let dir = tempdir_inside_codebase_dir();

    let Some(test_rollup) = new_test_rollup(
        dir.clone(),
        genesis_params,
        0,
        true,
        TEST_MAX_BATCH_SIZE,
        BlockProducingConfig::OnAnySubmit {
            block_wait_timeout_ms: None,
        },
        None,
    )
    .await
    else {
        // Docker issues, don't fail the test and just return early.
        return;
    };

    // Produce a few blocks to DA blocks to make sure there's a finalized slot after genesis.
    test_rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();
    sleep(Duration::from_millis(200)).await;
    test_rollup.da_service.set_delay_blobs_by(100).await;

    let client = test_rollup.api_client.clone();
    let mut slot_subscription = test_rollup.api_client.subscribe_slots().await.unwrap();
    for i in 0..100 {
        let tx = tx_set_value(&admin.private_key, i, i);
        client
            .send_raw_tx_to_sequencer_with_retry(&tx)
            .await
            .unwrap();

        test_rollup.da_service.produce_block_now().await.unwrap();
        slot_subscription.next().await;
    }

    test_rollup
        .da_service
        .produce_n_blocks_now(100)
        .await
        .unwrap();
    sleep(Duration::from_millis(200)).await;
    test_rollup.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn flaky_seq_back_pressure() {
    let (test_rollup, admin) = create_test_rollup(0, TEST_MAX_BATCH_SIZE).await;

    let Some(test_rollup) = test_rollup else {
        return;
    };

    test_rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();

    sleep(Duration::from_millis(200)).await;

    let client = test_rollup.api_client.clone();
    let tx = tx_set_value(&admin.private_key, 0, 9);

    client
        .accept_tx(&api_types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&tx),
        })
        .await
        .unwrap();

    // Pause block submission and produce some pending blocks.
    {
        test_rollup.da_service.set_blob_submission_pause().await;
        for _ in 0..sov_test_utils::TEST_MAX_CONCURRENT_BLOBS + 3 {
            test_rollup.da_service.produce_block_now().await.unwrap();
            sleep(Duration::from_millis(800)).await;
        }

        let tx = tx_set_value(&admin.private_key, 1, 9);
        let err = client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("The sequencer is waiting for the blob sender to be ready"));

        test_rollup.da_service.resume_blob_submission().await;
    }

    for _ in 0..5 {
        test_rollup.da_service.produce_block_now().await.unwrap();
        sleep(Duration::from_millis(800)).await;
    }
    sleep(Duration::from_millis(1000)).await;

    let tx = tx_set_value(&admin.private_key, 1, 9);
    client
        .send_raw_tx_to_sequencer_with_retry(&tx)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn seq_many_invalid_txs() {
    let (test_rollup, admin) = create_test_rollup(0, TEST_MAX_BATCH_SIZE).await;

    let Some(test_rollup) = test_rollup else {
        return;
    };

    test_rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();

    sleep(Duration::from_millis(200)).await;

    let client = test_rollup.api_client.clone();
    let tx = tx_set_value(&admin.private_key, 100, 0);

    client
        .send_raw_tx_to_sequencer_with_retry(&tx)
        .await
        .unwrap();

    let mut handles = Vec::default();
    for i in 0..100 {
        let client = client.clone();
        let da_service = test_rollup.da_service.clone();

        let tx = tx_set_value(&admin.private_key, 0, i);
        handles.push(tokio::spawn(async move {
            let res = client.send_raw_tx_to_sequencer_with_retry(&tx).await;
            da_service.produce_block_now().await.unwrap();
            res
        }));
    }

    test_rollup
        .da_service
        .produce_n_blocks_now(50)
        .await
        .unwrap();

    let results = future::join_all(handles).await;
    for res in results {
        assert!(res.unwrap().is_err());
    }
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
async fn query_historical_values() {
    let panicked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let panicked_ref = panicked.clone();
    let prev_hook = std::panic::take_hook();
    // Set a new panic hook
    std::panic::set_hook(Box::new(move |panic_info| {
        // Your custom panic handling logic here
        panicked_ref.store(true, std::sync::atomic::Ordering::SeqCst);
        prev_hook(panic_info);
    }));
    let tx_builder = |key| tx_set_value(&key, 0, 7);
    let assertions = |test_rollup| async move {
        query_set_value(&test_rollup, Some(2), 7).await.unwrap();
        query_set_value(&test_rollup, Some(0), 0).await.unwrap();
        query_set_value_by_slot_number(&test_rollup, Some(5), 7)
            .await
            .unwrap();
        query_set_value_by_slot_number(&test_rollup, Some(1), 0)
            .await
            .unwrap();
        // Query some future heights/slot numbers to be sure they don't panic
        assert!(query_set_value(&test_rollup, Some(100000), 0)
            .await
            .is_err());
        assert!(
            query_set_value_by_slot_number(&test_rollup, Some(100000), 0)
                .await
                .is_err()
        );
        test_rollup.shutdown().await.unwrap();
        assert!(
            !panicked.load(std::sync::atomic::Ordering::SeqCst),
            "Panicked during assertions"
        );
    };
    do_manual_block_production_test(tx_builder, assertions, 22222).await;
}

async fn do_manual_block_production_test<Fut: Future<Output = ()>>(
    tx_builder: impl Fn(Ed25519PrivateKey) -> RawTx,
    assertions: impl FnOnce(TestRollup<TestBlueprint>) -> Fut,
    port: u16,
) {
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
            c.rollup_prover_config = None;
            c.storage = dir;
            c.axum_port = port;
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
    let tx = tx_builder(admin.private_key.clone());
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

    // Add a delay to ensure that the sequencer has finished updating state on top of the new DA block. This ensures
    // that the block is visible, which is needed because we're running archival queries.
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(next.number, 5);
    assert_eq!(
        next.batches[0].txs[0].receipt.result,
        TxReceiptResult::Successful,
        "Replay in the sequencer was successful, but replay on the node failed: {:?}",
        next.batches[0],
    );

    assertions(test_rollup).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn events_are_returned_in_tx_response() {
    let (test_rollup, admin) = create_test_rollup(0, TEST_MAX_BATCH_SIZE).await;

    let Some(test_rollup) = test_rollup else {
        return;
    };

    // Produce a few blocks to DA blocks to make sure there's a finalized slot after genesis.
    test_rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();
    sleep(Duration::from_millis(200)).await;

    let client = test_rollup.api_client.clone();
    let tx = tx_set_value(&admin.private_key, 0, 7);
    let response = client
        .accept_tx(&api_types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&tx),
        })
        .await
        .unwrap();

    assert_eq!(response.data.events.len(), 1);
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
    let tx_builder = |key| tx_assert_visible_slot_number(&key, 0, 2);
    do_manual_block_production_test(tx_builder, |_| async {}, 22223).await;
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
            c.rollup_prover_config = None;
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
// TODO(@neysofu): unflaky it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Broken by removal of /batches endpoint, will be testable again when we add node/sequencer state comparision support"]
async fn flaky_test_hooks_state_is_visible() {
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
            c.rollup_prover_config = None;
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
    let (test_rollup, admin) = create_test_rollup(0, TEST_MAX_BATCH_SIZE).await;

    let Some(test_rollup) = test_rollup else {
        return;
    };

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

#[derive(BorshSerialize, BorshDeserialize, Clone)]
struct X {
    data: Vec<Vec<u8>>,
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

#[tokio::test(flavor = "multi_thread")]
async fn test_sequencer_state_and_node_state_matches() {
    let actions = vec![
        TestingAction::PauseUpdateStateExecution(true),
        TestingAction::AcceptTxs { count: 50 },
        // Query the state the sequencer has produced
        // Sequencer state updates are currently paused so we dont
        // update with state from the node.
        TestingAction::QuerySetValue,
        TestingAction::PauseUpdateStateExecution(false),
        // Produce a block to trigger an update_state operation
        // which will replace the state with that from the node.
        // It should produce the same result as our previous query
        TestingAction::NewDaSlot,
        // Give time for `update_state` to run
        // This should produce a batch containing previous transactions
        TestingAction::Sleep { duration_ms: 500 },
        // Trigger another block so we get updated state
        TestingAction::NewDaSlot,
        // give time for update state to run
        TestingAction::Sleep { duration_ms: 500 },
        // Should be unchanged
        TestingAction::QuerySetValue,
    ];

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn api_state_race_condition_regression2() {
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

async fn preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions: Vec<TestingAction>) {
    let mut genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    genesis_config.initial_sequencer.bond = genesis_config
        .initial_sequencer
        .bond
        .checked_mul(Amount::new(100))
        .unwrap();

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

    let dir = tempdir_inside_codebase_dir();

    let Some(test_rollup) = new_test_rollup(
        dir.clone(),
        genesis_params,
        0,
        false,
        TEST_MAX_BATCH_SIZE,
        DEFAULT_BLOCK_PRODUCING_CONFIG,
        Some(RollupProverConfig::Skip),
    )
    .await
    else {
        // Docker issues, don't fail the test and just return early.
        return;
    };

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
        TestingAction::PauseUpdateStateExecution(v) => pause_update_state::set(v),
        TestingAction::Sleep { duration_ms } => {
            sleep(Duration::from_millis(duration_ms)).await;
        }
        TestingAction::Restart => {
            return test_rollup.restart().await;
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
        TestingAction::NewDaSlot { .. } => {
            test_rollup.da_service.produce_block_now().await.unwrap();
        }
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
    rollup_height: Option<u64>,
    expected: u64,
) -> anyhow::Result<()> {
    query_set_value_helper(
        test_rollup,
        rollup_height.map(|n| format!("rollup_height={}", n)),
        expected,
    )
    .await
}

async fn query_set_value_by_slot_number(
    test_rollup: &TestRollup<TestBlueprint>,
    slot_number: Option<u64>,
    expected: u64,
) -> anyhow::Result<()> {
    query_set_value_helper(
        test_rollup,
        slot_number.map(|n| format!("slot_number={}", n)),
        expected,
    )
    .await
}

async fn query_set_value_helper(
    test_rollup: &TestRollup<TestBlueprint>,
    query_param: Option<String>,
    expected: u64,
) -> anyhow::Result<()> {
    let url = format!(
        "/modules/value-setter/state/value{}",
        if let Some(query_param) = query_param {
            format!("?{}", query_param)
        } else {
            "".to_string()
        }
    );
    let response = test_rollup
        .client
        .query_rest_endpoint::<ResponseObject<serde_json::Value>>(&url)
        .await?;

    debug!(?response, "Querying value");

    let found_value = response
        .data
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No data found. {:?}", response))?["value"]
        .as_u64()
        .unwrap_or_default();

    anyhow::ensure!(found_value == expected);

    Ok(())
}

fn tx_set_value(key: &Ed25519PrivateKey, nonce: u64, value_to_set: u64) -> RawTx {
    tx_set_value_with_gas(
        key,
        nonce,
        value_to_set,
        None,
        sov_test_utils::TEST_DEFAULT_MAX_FEE,
    )
}

fn tx_set_value_with_gas(
    key: &Ed25519PrivateKey,
    nonce: u64,
    value_to_set: u64,
    gas: Option<GasUnit<2>>,
    max_fee: Amount,
) -> RawTx {
    let msg = <TestRuntime<TestSpec> as DispatchCall>::Decodable::ValueSetter(
        sov_value_setter::CallMessage::SetValue {
            value: value_to_set as u32,
            gas,
        },
    );

    encode_call_with_fee(key, nonce, &msg, max_fee)
}

fn tx_set_many_values(key: &Ed25519PrivateKey, nonce: u64, values_to_set: Vec<u8>) -> RawTx {
    let msg = <TestRuntime<TestSpec> as DispatchCall>::Decodable::ValueSetter(
        sov_value_setter::CallMessage::SetManyValues(values_to_set),
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

fn encode_call_with_fee(
    key: &Ed25519PrivateKey,
    nonce: u64,
    call_message: &<TestRuntime<TestSpec> as DispatchCall>::Decodable,
    max_fee: Amount,
) -> RawTx {
    let mut tx_details = default_test_tx_details();
    tx_details.max_fee = max_fee;
    let tx = test_signed_transaction::<TestRuntime<TestSpec>, TestSpec>(
        key,
        call_message,
        nonce,
        &<TestRuntime<TestSpec> as Runtime<TestSpec>>::CHAIN_HASH,
        tx_details,
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
