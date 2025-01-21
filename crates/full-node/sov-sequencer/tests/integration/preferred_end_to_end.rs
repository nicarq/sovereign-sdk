//! Integration tests for the preferred sequencer that use [`RollupBuilder`] and
//! thus test sequencer + node interactions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use proptest::sample::size_range;
use sov_api_spec::types as api_types;
use sov_api_spec::types::PublishBatchBody;
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
use sov_stf_runner::processes::RollupProverConfig;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, TestRollup};
use sov_test_utils::{
    default_test_signed_transaction, generate_optimistic_runtime_with_kernel, RtAgnosticBlueprint,
    TestSpec,
};
use sov_value_setter::{ValueSetter, ValueSetterConfig};
use test_strategy::Arbitrary;
use tokio::time::sleep;
use tracing::{debug, info};

use crate::utils::generate_txs;

generate_optimistic_runtime_with_kernel!(
    TestRuntime <=
    kernel_type: sov_kernels::soft_confirmations::SoftConfirmationsKernel<'a, S>,
    value_setter: ValueSetter<S>,
    paymaster: Paymaster<S>
);

type TestBlueprint = RtAgnosticBlueprint<TestSpec, TestRuntime<TestSpec>>;

/// All the interesting "things" that can happen during sequencer operations, and to
/// which the sequencer ought to know how to respond.
#[derive(Debug, Clone, Arbitrary)]
enum TestingAction {
    #[weight(0)] // Never generated automatically, but useful for debugging.
    Sleep { duration_ms: u64 },
    /// The node is shutdown and restarted, to catch possible losses of
    /// soft-confirmed transactions and state initialization bugs.
    Restart,
    /// A client submits a valid transaction to be included in the next batch,
    /// and for which a soft confirmation ought to be provided immediately.
    #[weight(5)] // Make it more likely to be picked (this is where all juicy stuff happens)
    AcceptTx,
    /// A client submits an **invalid** transactions, asking for it to be
    /// included in the next batch (it won't, as it's invalid).
    #[weight(2)]
    TryAcceptBadTx { invalid_reason: InvalidGeneration },
    #[weight(3)]
    ProduceBatch {
        #[strategy(0..5usize)]
        num_txs: usize,
    },
    /// A client queries the nonce for a given address.
    ///
    /// This is an easy and effective way for us to check that all pending
    /// transactions have actually been processed by the sequencer, and that
    /// its state changes are visible to REST API clients.
    #[weight(8)]
    QuerySetValue,
    /// Like [`Self::QuerySetValue`], but historical queries.
    ///
    /// FIXME(@neysofu): historical queries only work for node-processed slots,
    /// and not soft confirmations. This is arguably fine, but this test feature
    /// doesn't take that into account and is currently broken.
    #[weight(0)]
    #[allow(dead_code)]
    QuerySetValueHistorical,
    /// The node will process the latest DA slot, and inform the sequencer about
    /// it.
    ///
    /// TODO(@neysofu)
    NewDaSlot {
        #[any(size_range(0..1).lift())]
        _non_preferred_batches: Vec<()>,
    },
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
    minimum_profit_per_tx: u64,
) -> TestRollup<TestBlueprint> {
    const FINALIZATION_BLOCKS: u32 = 3;
    let sequencer_addr = genesis_params.runtime.sequencer_registry.seq_da_address;

    RollupBuilder::<TestBlueprint>::new(
        GenesisSource::CustomParams(genesis_params),
        BlockProducingConfig::OnAnySubmit,
        FINALIZATION_BLOCKS,
        minimum_profit_per_tx,
        Default::default(),
    )
    .set_config(|c| {
        c.rollup_prover_config = RollupProverConfig::Skip;
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
    current_slot_number: SlotNumber,
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
            PaymasterConfig::default(),
        );
    let genesis_params = GenesisParams {
        runtime: rt_genesis_config.clone(),
    };

    let dir = Arc::new(tempfile::tempdir().unwrap());

    let test_rollup = new_test_rollup(dir.clone(), genesis_params, 1).await;

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
            PaymasterConfig::default(),
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
            0,
            Default::default(),
        )
        .set_config(|c| {
            c.rollup_prover_config = RollupProverConfig::Skip;
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

    // Run one slot. This causes both begin and end slot hooks to be run.
    test_rollup
        .api_client
        .publish_batch(&PublishBatchBody {
            transactions: vec![],
        })
        .await
        .unwrap();

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
                "/modules/value-setter/state/{}-hook-count",
                hook_name
            ))
            .await
            .unwrap()
            .data
            .unwrap()
            .value
    };

    let begin_slot_count = query_hook_counter("begin-slot").await;
    assert_eq!(begin_slot_count, 2);

    //  since we haven't finished building this batch, the end slot hook hasn't been run - so its value is still 1
    let end_slot_count = query_hook_counter("end-slot").await;
    assert_eq!(end_slot_count, 1);
    // The finalize hook runs during genesis, so it should have been run twice
    let finalize_count = query_hook_counter("finalize").await;
    assert_eq!(finalize_count, 2);

    // Finish the in-progress batch (and start the next one). Now the end slot hook should have been run again.
    test_rollup
        .api_client
        .publish_batch(&PublishBatchBody {
            transactions: vec![],
        })
        .await
        .unwrap();
    let end_slot_count = query_hook_counter("end-slot").await;
    assert_eq!(end_slot_count, 2);
    // The finalize hook should also have been run again
    let finalize_count = query_hook_counter("finalize").await;
    assert_eq!(finalize_count, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_production_and_accept_tx() {
    let mut actions = vec![];
    for i in 1..20 {
        actions.push(TestingAction::ProduceBatch { num_txs: i });
        actions.push(TestingAction::AcceptTx);
    }

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
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
        TestingAction::ProduceBatch { num_txs: 4 },
        TestingAction::ProduceBatch { num_txs: 4 },
        TestingAction::ProduceBatch { num_txs: 4 },
        TestingAction::ProduceBatch { num_txs: 4 },
        TestingAction::ProduceBatch { num_txs: 4 },
        TestingAction::ProduceBatch { num_txs: 4 },
        TestingAction::QuerySetValue,
        TestingAction::ProduceBatch { num_txs: 4 },
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
        TestingAction::ProduceBatch { num_txs: 1 },
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
        TestingAction::NewDaSlot {
            _non_preferred_batches: vec![],
        },
        TestingAction::QuerySetValue,
        TestingAction::QuerySetValue,
        TestingAction::QuerySetValue,
        TestingAction::QuerySetValue,
        TestingAction::ProduceBatch { num_txs: 4 },
        TestingAction::TryAcceptBadTx {
            invalid_reason: InvalidGeneration::TooOld,
        },
        TestingAction::ProduceBatch { num_txs: 4 },
        TestingAction::QuerySetValue,
    ];

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_after_big_batch_regression() {
    let actions = vec![
        TestingAction::ProduceBatch { num_txs: 1 },
        TestingAction::ProduceBatch { num_txs: 5 },
        TestingAction::AcceptTx,
        TestingAction::Restart,
        TestingAction::ProduceBatch { num_txs: 10 },
        TestingAction::Restart,
        TestingAction::AcceptTx,
    ];

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_production_with_immediate_finalization() {
    let actions = vec![
        TestingAction::ProduceBatch { num_txs: 1 },
        TestingAction::ProduceBatch { num_txs: 50 },
        TestingAction::Restart,
        TestingAction::AcceptTx,
        TestingAction::Sleep { duration_ms: 50 },
        TestingAction::Restart,
        TestingAction::ProduceBatch { num_txs: 50 },
        TestingAction::Restart,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::AcceptTx,
        TestingAction::ProduceBatch { num_txs: 3 },
        TestingAction::ProduceBatch { num_txs: 50 },
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
            PaymasterConfig::default(),
        );
    let genesis_params = GenesisParams {
        runtime: rt_genesis_config.clone(),
    };

    let dir = Arc::new(tempfile::tempdir().unwrap());

    let test_rollup = new_test_rollup(dir.clone(), genesis_params, 0).await;
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
        TestingAction::ProduceBatch { num_txs } => {
            let mut txs = vec![];
            for _ in 0..num_txs {
                let tx = tx_set_value(
                    key,
                    test_state.next_generation,
                    test_state.current_value + 1,
                );
                test_state.current_value += 1;

                txs.push(tx.data);
            }
            test_state.next_generation += 1;

            test_rollup.client.publish_batch(txs, true).await?;
            test_state.current_slot_number.incr();
            test_state
                .value_by_slot_number
                .insert(test_state.current_slot_number, test_state.current_value);
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
    use sov_mock_da::BlockProducingConfig;
    use sov_modules_stf_blueprint::GenesisParams;
    use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder};
    use sov_test_utils::RtAgnosticBlueprint;

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
            BlockProducingConfig::Periodic,
            0,
            0,
            Default::default(),
        )
        .set_config(|conf| {
            conf.batch_builder_config =
                sov_sequencer::BatchBuilderConfig::Preferred(Default::default());
        })
        .start()
        .await
        .unwrap();
    }
}
