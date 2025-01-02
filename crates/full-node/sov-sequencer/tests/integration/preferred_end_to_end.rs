//! Integration tests for the preferred sequencer that use [`RollupBuilder`] and
//! thus test sequencer + node interactions.

use std::sync::Arc;
use std::time::Duration;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use proptest::sample::size_range;
use sov_api_spec::types as api_types;
use sov_mock_da::BlockProducingConfig;
use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
use sov_modules_api::prelude::*;
use sov_modules_api::{DispatchCall, RawTx, Runtime};
use sov_modules_stf_blueprint::GenesisParams;
use sov_rest_utils::ResponseObject;
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

use crate::utils::generate_txs;

generate_optimistic_runtime_with_kernel!(
    TestRuntime <=
    kernel_type: sov_kernels::soft_confirmations::SoftConfirmationsKernel<'a, S>,
    value_setter: ValueSetter<S>
);

type TestBlueprint = RtAgnosticBlueprint<TestSpec, TestRuntime<TestSpec>>;

#[derive(Debug, Clone, Arbitrary)]
enum TestingAction {
    /// A client queries the nonce for a given address.
    ///
    /// This is an easy and effective way for us to check that all pending
    /// transactions have actually been processed by the sequencer, and that
    /// its state changes are visible to REST API clients.
    QuerySetValue,
    /// The sequencer is shutdown and restarted, to catch possible losses of
    /// soft-confirmed transactions and state initialization bugs.
    Restart,
    /// A client submits a valid transaction to be included in the next batch,
    /// and for which a soft confirmation ought to be provided immediately.
    #[weight(3)] // Make it more likely to be picked (this is where all juicy stuff happens)
    AcceptTx,
    /// A client submits an **invalid** transactions, asking for it to be
    /// included in the next batch (it won't, as it's invalid).
    TryAcceptBadTx { wrong_nonce: WrongNonce },
    ProduceBatch {
        #[strategy(0..5usize)]
        num_txs: usize,
    },
    /// The node will process the latest DA slot, and inform the sequencer about
    /// it.
    NewDaSlot {
        #[any(size_range(0..1).lift())]
        _non_preferred_batches: Vec<()>,
    },
}

/// A nonce that is off by one compared to the expected one.
#[derive(Debug, Clone, Arbitrary)]
enum WrongNonce {
    PlusOne,
    MinusOne,
    Zero,
}

async fn new_test_rollup(
    dir: Arc<tempfile::TempDir>,
    genesis_params: GenesisParams<<TestRuntime<TestSpec> as Runtime<TestSpec>>::GenesisConfig>,
    minimum_profit_per_tx: u64,
) -> TestRollup<TestBlueprint> {
    const FINALIZATION_BLOCKS: u32 = 10;
    let sequencer_addr = genesis_params.runtime.sequencer_registry.seq_da_address;

    RollupBuilder::<TestBlueprint>::new(
        GenesisSource::CustomParams(genesis_params),
        BlockProducingConfig::Periodic,
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

#[should_panic] // FIXME(@neysofu)
#[test]
fn outer_preferred_sequencer_is_resistant_to_miscellaneous_edge_cases() {
    use proptest::prelude::*;
    use proptest::test_runner::{Config, TestRunner};

    let mut runner = TestRunner::new(Config::with_cases(10));
    let result = runner.run(
        &proptest::collection::vec(any::<TestingAction>(), 0..100),
        |actions| {
            let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            tokio_runtime.block_on(async {
                preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
            });

            Ok(())
        },
    );

    result.unwrap();
}

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

#[should_panic] // FIXME(@neysofu)
#[tokio::test(flavor = "multi_thread")]
async fn produce_batch_restart_then_accept_tx() {
    let actions = vec![
        TestingAction::ProduceBatch { num_txs: 1 },
        TestingAction::Restart,
        TestingAction::AcceptTx,
    ];

    preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions).await;
}

async fn preferred_sequencer_is_resistant_to_miscellaneous_edge_cases(actions: Vec<TestingAction>) {
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let admin = genesis_config.additional_accounts[0].clone();

    let rt_genesis_config =
        <TestRuntime<TestSpec> as Runtime<TestSpec>>::GenesisConfig::from_minimal_config(
            genesis_config.into(),
            ValueSetterConfig {
                admin: admin.address(),
            },
        );
    let genesis_params = GenesisParams {
        runtime: rt_genesis_config.clone(),
    };

    let dir = Arc::new(tempfile::tempdir().unwrap());

    let test_rollup = new_test_rollup(dir.clone(), genesis_params, 0).await;
    let client = test_rollup.api_client.clone();

    let mut next_nonce = 0u64;
    {
        let txs = generate_txs(admin.private_key.clone()).clone();
        for tx in txs {
            client
                .accept_tx(&api_types::AcceptTxBody {
                    body: BASE64_STANDARD.encode(&tx.raw_tx),
                })
                .await
                .unwrap();
            next_nonce += 1;
        }
    }

    // initialize nonce value
    {
        let tx = tx_set_value(&admin.private_key, next_nonce, next_nonce);
        client
            .accept_tx(&api_types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();
        next_nonce += 1;
    }

    let mut test_rollup = Some(test_rollup);

    for (i, action) in actions.iter().enumerate() {
        let new_test_rollup_res = run_action_against_test_rollup(
            test_rollup.take().unwrap(),
            rt_genesis_config.clone(),
            &admin.private_key,
            action.clone(),
            &mut next_nonce,
        )
        .await;

        match new_test_rollup_res {
            Ok(new_test_rollup) => test_rollup = Some(new_test_rollup),
            Err(e) => {
                println!("Action history: {:#?}", actions[..=i].to_vec());
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
    next_nonce: &mut u64,
) -> anyhow::Result<TestRollup<TestBlueprint>> {
    assert!(*next_nonce > 0);

    let client = test_rollup.api_client.clone();

    match action {
        TestingAction::Restart => {
            let storage_dir = test_rollup.storage.clone();
            let genesis_params = GenesisParams {
                runtime: rt_genesis_params,
            };

            test_rollup.shutdown().await?;

            sleep(Duration::from_millis(100)).await;

            return Ok(new_test_rollup(storage_dir, genesis_params, 0).await);
        }
        TestingAction::TryAcceptBadTx { wrong_nonce } => {
            let bad_nonce = match wrong_nonce {
                WrongNonce::MinusOne => *next_nonce - 1,
                WrongNonce::PlusOne => *next_nonce + 1,
                WrongNonce::Zero => 0,
            };
            let tx = tx_set_value(key, bad_nonce, *next_nonce);

            anyhow::ensure!(client
                .accept_tx(&api_types::AcceptTxBody {
                    body: BASE64_STANDARD.encode(&tx),
                })
                .await
                .is_err());
        }
        TestingAction::AcceptTx => {
            let tx = tx_set_value(key, *next_nonce, *next_nonce);
            *next_nonce += 1;
            client
                .accept_tx(&api_types::AcceptTxBody {
                    body: BASE64_STANDARD.encode(&tx),
                })
                .await?;
        }
        TestingAction::ProduceBatch { num_txs } => {
            let mut txs = vec![];
            for _ in 0..num_txs {
                let tx = tx_set_value(key, *next_nonce, *next_nonce);
                *next_nonce += 1;

                txs.push(BASE64_STANDARD.encode(tx.data));
            }

            client
                .publish_batch(&api_types::PublishBatchBody { transactions: txs })
                .await?;
        }
        TestingAction::NewDaSlot { .. } => {}
        TestingAction::QuerySetValue => {
            let response = test_rollup
                .client
                .query_rest_endpoint::<ResponseObject<serde_json::Value>>(
                    "/modules/value-setter/state/value",
                )
                .await?;

            let value_opt = response.data.and_then(|data| data["value"].as_u64());
            let last_used_nonce = next_nonce.saturating_sub(1);
            anyhow::ensure!(value_opt.unwrap_or_default() == last_used_nonce);
        }
    }

    Ok(test_rollup)
}

fn tx_set_value(key: &Ed25519PrivateKey, nonce: u64, value_to_set: u64) -> RawTx {
    let msg = <TestRuntime<TestSpec> as DispatchCall>::Decodable::ValueSetter(
        sov_value_setter::CallMessage::SetValue(value_to_set as u32),
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
