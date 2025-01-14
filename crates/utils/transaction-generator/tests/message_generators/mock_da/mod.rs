//! Uses the transaction generator to build transactions using the MockDA

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt, TryStreamExt};
use progenitor_client::ResponseValue;
use sov_mock_da::storable::layer::StorableMockDaLayer;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::{BlockProducingConfig, MockFee};
use sov_modules_api::prelude::arbitrary::{Arbitrary, Unstructured};
use sov_modules_api::prelude::tokio;
use sov_modules_api::prelude::tokio::time::timeout;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{CryptoSpec, FullyBakedTx, HexHash, Runtime, Spec};
use sov_node_client::NodeClient;
use sov_paymaster::{
    PayeePolicy, PayerGenesisConfig, Paymaster, PaymasterConfig, PaymasterPolicyInitializer,
    SafeVec,
};
use sov_rollup_interface::node::da::{DaService, SubmitBlobReceipt};
use sov_rollup_interface::node::ledger_api::IncludeChildren;
use sov_sequencer::batch_builders::preferred::PreferredBatchBuilderConfig;
use sov_sequencer::BatchBuilderConfig;
use sov_test_utils::ledger_db::sov_api_spec::types::{
    AcceptTxResponse, Slot, SubmitBatchReceipt, TxReceiptResult,
};
use sov_test_utils::runtime::genesis::zk::config::HighLevelZkGenesisConfig;
use sov_test_utils::runtime::genesis::zk::MinimalZkGenesisConfig;
use sov_test_utils::runtime::sov_blob_storage::config_deferred_slots_count;
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, RollupProverConfig, TestRollup};
use sov_test_utils::{
    generate_runtime, AsUser, RtAgnosticBlueprint, TestProver, TestSequencer, TestSpec as S,
    TestUser, TransactionType, TEST_DEFAULT_USER_BALANCE,
};
use sov_transaction_generator::generators::basic::{BasicChangeLogEntry, BasicClientConfig};
use sov_transaction_generator::interface::rng_utils::get_random_bytes;
use sov_transaction_generator::interface::{MessageValidity, Percent};
use sov_transaction_generator::{assert_logs_against_state, Distribution, GeneratedMessage};
use sov_value_setter::{ValueSetter, ValueSetterConfig};

use crate::{
    plain_tx_with_default_details, setup_harness, ModulesToUse, TestGenerator,
    MAX_VEC_LEN_VALUE_SETTER, USER_BALANCE,
};

/// The number of transactions to generate.
pub const DEFAULT_BLOCK_PRODUCING_CONFIG: BlockProducingConfig = BlockProducingConfig::Periodic;
pub const DEFAULT_BLOCK_TIME_MS: u64 = 500;
pub const DEFAULT_BLOCK_TIME: Duration = Duration::from_millis(DEFAULT_BLOCK_TIME_MS);
pub const DEFAULT_FINALIZATION_BLOCKS: u32 = 5;
pub const DEFAULT_TXS_PER_BATCH: u64 = 10;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

generate_runtime! {
    name: TestRuntime,
    modules: [paymaster: Paymaster<S>, value_setter: ValueSetter<S>],
    operating_mode: sov_modules_api::runtime::OperatingMode::Zk,
    minimal_genesis_config_type: MinimalZkGenesisConfig<S>,
    gas_enforcer: paymaster: Paymaster<S>,
    runtime_trait_impl_bounds: [],
    kernel_type: sov_kernels::soft_confirmations::SoftConfirmationsKernel<'a, S>
}

type RT = TestRuntime<S>;
type GeneratorOutput = GeneratedMessage<S, TestRuntimeCall<S>, BasicChangeLogEntry<S>>;

pub struct Setup {
    #[allow(dead_code)]
    pub user: TestUser<S>,
    /// A user who is pre-registered as a payer for [`sequencer`]
    #[allow(dead_code)]
    pub payer: TestUser<S>,
    /// The pre-registered sequencer
    pub sequencer: TestSequencer<S>,
    /// The pre-registered prover
    pub prover: TestProver<S>,
    /// The admin user of [`ValueSetter`] module
    pub value_setter_admin: TestUser<S>,
    #[allow(missing_docs)]
    pub genesis_config: GenesisConfig<S>,
}

fn setup_roles_and_config(user_balance: u64) -> Setup {
    let genesis_config = HighLevelZkGenesisConfig::generate()
        .add_accounts_with_balance(2, TEST_DEFAULT_USER_BALANCE)
        .add_accounts_with_balance(2, user_balance);

    let sequencer = genesis_config.initial_sequencer.clone();
    let prover = genesis_config.initial_prover.clone();
    let payer = genesis_config.additional_accounts.first().unwrap().clone();
    let admin = genesis_config.additional_accounts.get(1).unwrap().clone();
    let user = genesis_config.additional_accounts.get(2).unwrap().clone();
    let genesis_config = GenesisConfig::from_minimal_config(
        genesis_config.into(),
        PaymasterConfig {
            payers: [PayerGenesisConfig {
                payer_address: payer.address(),
                policy: PaymasterPolicyInitializer {
                    default_payee_policy: PayeePolicy::Allow {
                        max_fee: None,
                        gas_limit: None,
                        max_gas_price: None,
                    },
                    payees: SafeVec::new(),
                    authorized_sequencers: sov_paymaster::AuthorizedSequencers::All,
                    authorized_updaters: [payer.address()].as_ref().try_into().unwrap(),
                },
                sequencers_to_register: [sequencer.da_address].as_ref().try_into().unwrap(),
            }]
            .as_ref()
            .try_into()
            .unwrap(),
        },
        ValueSetterConfig {
            admin: admin.address(),
        },
    );
    Setup {
        payer,
        sequencer,
        prover,
        user,
        genesis_config,
        value_setter_admin: admin,
    }
}

type RollupBlueprint = RtAgnosticBlueprint<S, RT>;
type TestRollupBuilder = RollupBuilder<RollupBlueprint>;

struct TxBuilder {
    nonces: HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    /// A list of transactions sent we are waiting for inclusion on DA
    txs_to_wait_for: Vec<HexHash>,
    /// For now we use a slot subscription until we can reliably receive [`TxStatus::Processed`] from the full-node
    slots_subscription: Pin<Box<dyn Stream<Item = Result<Slot, anyhow::Error>> + Send>>,
    config: TxBuilderConfig,
}

struct TxBuilderConfig {
    pub modules: Distribution<ModulesToUse>,
    pub validity: Distribution<MessageValidity>,
}

impl TxBuilder {
    async fn new(config: TxBuilderConfig, client: &NodeClient) -> Self {
        Self {
            nonces: Default::default(),
            txs_to_wait_for: Default::default(),
            slots_subscription: client
                .client
                .subscribe_finalized_slots_with_children(IncludeChildren::new(true))
                .await
                .expect("Impossible to subscribe to the slots"),
            config,
        }
    }

    /// Builds a transaction out of the transaction generator's output
    fn sign_generator_output(&mut self, gen_output: &GeneratorOutput) -> Transaction<RT, S> {
        let TransactionType::Plain {
            message,
            key,
            details,
        } = plain_tx_with_default_details::<RT>(gen_output)
        else {
            panic!("The method `plain_tx_with_default_details` should return a plain transaction!");
        };

        TransactionType::<RT, S>::sign(message, key, &RT::CHAIN_HASH, details, &mut self.nonces)
    }

    fn bake_generator_output(&mut self, gen_output: &GeneratorOutput) -> FullyBakedTx {
        plain_tx_with_default_details::<RT>(gen_output)
            .to_serialized_authenticated_tx(&mut self.nonces)
    }

    async fn produce_and_publish_batch(
        &mut self,
        num_txs_in_batch: u64,
        generator: &mut TestGenerator<RT>,
        client: &NodeClient,
        u: &mut Unstructured<'_>,
    ) -> anyhow::Result<(Vec<GeneratorOutput>, SubmitBatchReceipt)> {
        let generator_outputs = self.generate_outputs(num_txs_in_batch, generator, u);

        let txs: Vec<_> = generator_outputs
            .iter()
            .map(|output| self.sign_generator_output(output))
            .collect();

        let batch_result = self.publish_transactions(txs, client).await?;

        Ok((generator_outputs, batch_result))
    }

    // TODO In future will be removed and `Self::produce_and_publish_batch` will be used against TestSequencer
    async fn produce_and_publish_batch_directly(
        &mut self,
        num_txs_in_batch: u64,
        generator: &mut TestGenerator<RT>,
        da_service: &StorableMockDaService,
        u: &mut Unstructured<'_>,
    ) -> anyhow::Result<(
        Vec<GeneratorOutput>,
        SubmitBlobReceipt<sov_mock_da::MockHash>,
    )> {
        let generator_outputs = self.generate_outputs(num_txs_in_batch, generator, u);

        let batch: Vec<FullyBakedTx> = generator_outputs
            .iter()
            .map(|output| self.bake_generator_output(output))
            .collect();
        // TODO: Add hash to wait for those txs too

        let serialize_batch = borsh::to_vec(&batch)?;

        let receipt = da_service
            .send_transaction(&serialize_batch, MockFee::zero())
            .await
            .await??;

        Ok((generator_outputs, receipt))
    }

    fn generate_outputs(
        &mut self,
        num_txs_in_batch: u64,
        generator: &mut TestGenerator<RT>,
        u: &mut Unstructured<'_>,
    ) -> Vec<GeneratorOutput> {
        let modules = self.config.modules.clone().map_values(&mut |module| {
            module.select::<RT>(
                generator.bank_harness.clone(),
                generator.value_setter_harness.clone(),
            )
        });
        (0..num_txs_in_batch)
            .map(|_| {
                let validity = self
                    .config
                    .validity
                    .select_value(u)
                    .expect("Ran out of randomness");
                generator.generate(&modules, *validity)
            })
            .collect()
    }
    async fn sign_and_send_generator_output(
        &mut self,
        gen_output: &GeneratorOutput,
        client: &NodeClient,
    ) -> Result<ResponseValue<AcceptTxResponse>, anyhow::Error> {
        let tx = self.sign_generator_output(gen_output);
        let result = client
            .client
            .send_txs_to_sequencer(&[tx])
            .await?
            .pop()
            .unwrap();

        Ok(result)
    }

    async fn publish_transactions(
        &mut self,
        txs: Vec<Transaction<RT, S>>,
        client: &NodeClient,
    ) -> anyhow::Result<SubmitBatchReceipt> {
        let batch_result = client
            .client
            .publish_batch_with_serialized_txs(&txs)
            .await?;

        for tx_hash in &batch_result.tx_hashes {
            let tx_hash = tx_hash.parse().expect("Invalid tx hash");
            self.txs_to_wait_for.push(tx_hash);
        }

        Ok(batch_result)
    }

    async fn wait_for_results(&mut self) -> anyhow::Result<()> {
        while !self.txs_to_wait_for.is_empty() {
            let Some(next_slot) = self.slots_subscription.try_next().await? else {
                continue;
            };
            for batch in next_slot.batches {
                for tx in batch.txs {
                    let parsed_hash: HexHash =
                        tx.hash.parse().expect("Impossible to parse tx_hash!");

                    if let Some(pos) = self
                        .txs_to_wait_for
                        .iter()
                        .position(|hash| hash == &parsed_hash)
                    {
                        assert_eq!(
                            tx.receipt.result,
                            TxReceiptResult::Successful,
                            "The transaction should be successful"
                        );

                        self.txs_to_wait_for.remove(pos);
                    }
                }
            }
        }

        Ok(())
    }
}

async fn test_with_modules(
    modules: Distribution<ModulesToUse>,
    validity: Distribution<MessageValidity>,
) -> (TestRollup<RollupBlueprint>, Vec<GeneratorOutput>) {
    let random_bytes = get_random_bytes(100_000_000, 1);
    let u = &mut Unstructured::new(&random_bytes[..]);

    let setup = setup_roles_and_config(USER_BALANCE);
    let mut generator = setup_harness(
        Percent::one_hundred(),
        &setup.value_setter_admin,
        MAX_VEC_LEN_VALUE_SETTER,
        &modules,
    );

    let rollup_builder = TestRollupBuilder::new(
        GenesisSource::CustomParams(setup.genesis_config.into_genesis_params()),
        DEFAULT_BLOCK_PRODUCING_CONFIG,
        DEFAULT_FINALIZATION_BLOCKS,
        0,
        Default::default(),
    )
    .set_config(|config| {
        config.batch_builder_config = BatchBuilderConfig::Preferred(PreferredBatchBuilderConfig {
            minimum_profit_per_tx: 0,
        });
        config.prover_address = setup.prover.user_info.address().to_string();
    })
    .set_da_config(|da_config| {
        da_config.block_time_ms = DEFAULT_BLOCK_TIME_MS;
        da_config.sender_address = setup.sequencer.da_address;
    });

    let rollup = rollup_builder
        .start()
        .await
        .expect("Impossible to start rollup");

    let mut tx_builder =
        TxBuilder::new(TxBuilderConfig { modules, validity }, &rollup.client).await;

    // Execute initial transaction if there is one
    if let Some(init_tx) = generator.initial_transaction.take() {
        tx_builder
            .sign_and_send_generator_output(&init_tx, &rollup.client)
            .await
            .unwrap();
    }

    let (outputs, _) = tx_builder
        .produce_and_publish_batch(DEFAULT_TXS_PER_BATCH, &mut generator, &rollup.client, u)
        .await
        .unwrap();

    timeout(DEFAULT_TIMEOUT, tx_builder.wait_for_results())
        .await
        .expect("Timed out while waiting for transactions to finish executing")
        .expect("Some transactions where not correctly executed!");

    (rollup, outputs)
}

// This tests shows how several sequencers can be tested on the same MockDa.
// For flexibility, block production is manual.
// Scenario:
//   0. Initialize [`StorableMockDaLayer`] and keep Arc of it.
//   1. Configure and start rollup with the preferred sequencer.
//      Test passes`Arc<StorableMockDaLayer>` to the configuration of it.
//   2. Register additional standard sequencers
//   3. Configure additional [`StorableMockDaService`] (! Service) and pass the same `Arc<StorableMockDaLayer>` to it
//   4. There's a loop, where the first batch is submitted to rollup and then another batch submitted directly to DA layer.
//      This allows having 2 blobs in the same block.
//   5. In each iteration of the loop, the test generated [`DEFERRED_SLOTS_COUNT`] number of DA blocks, so both batches are executed,
//      and the following iteration of the loop has the correct state, as if it has been submitting to a single sequencer.
//      That's because the current test generator does not split transactions in any way.
async fn test_several_sequencers(
    modules: Distribution<ModulesToUse>,
    validity: Distribution<MessageValidity>,
    transactions_per_batch: u64,
) -> anyhow::Result<()> {
    let main_loop_iterations = 2;
    let random_bytes = get_random_bytes(1_000_000, 31337);
    let deferred_blocks_count = config_deferred_slots_count();
    let finalization = DEFAULT_FINALIZATION_BLOCKS;
    let blocks_till_result = deferred_blocks_count as u32 + 2;
    let u = &mut Unstructured::new(&random_bytes[..]);

    let mut setup = setup_roles_and_config(USER_BALANCE);
    let mut generator = setup_harness(
        Percent::one_hundred(),
        &setup.value_setter_admin,
        MAX_VEC_LEN_VALUE_SETTER,
        &modules,
    );
    let second_seq_user: TestUser<S> =
        TestUser::generate(setup.genesis_config.sequencer_registry.seq_bond * 3);
    let third_seq_user: TestUser<S> =
        TestUser::generate(setup.genesis_config.sequencer_registry.seq_bond * 3);
    for u in [&second_seq_user, &third_seq_user] {
        setup
            .genesis_config
            .bank
            .gas_token_config
            .address_and_balances
            .push((u.address(), u.balance()));
    }
    let regular_rollup_da_address = Arbitrary::arbitrary(u)?;
    let third_rollup_da_address = Arbitrary::arbitrary(u)?;
    let da_layer = Arc::new(tokio::sync::RwLock::new(
        StorableMockDaLayer::new_in_memory(finalization).await?,
    ));

    let preferred_rollup_builder = TestRollupBuilder::new(
        GenesisSource::CustomParams(setup.genesis_config.clone().into_genesis_params()),
        BlockProducingConfig::Manual,
        finalization,
        0,
        Default::default(),
    )
    .set_config(|config| {
        config.batch_builder_config = BatchBuilderConfig::Preferred(PreferredBatchBuilderConfig {
            minimum_profit_per_tx: 0,
        });
        config.rollup_prover_config = RollupProverConfig::Skip;
        config.prover_address = setup.prover.user_info.address().to_string();
        // // Setting very high aggregated proof jump to eliminate non-batches appear on DA.
        // // Can be removed later when tests are stabilized.
        config.aggregated_proof_block_jump = 3;
    })
    .set_da_config(|da_config| {
        da_config.block_time_ms = DEFAULT_BLOCK_TIME_MS;
        da_config.sender_address = setup.sequencer.da_address;
        da_config.da_layer = Some(da_layer.clone());
    });

    let rollup = preferred_rollup_builder
        .start()
        .await
        .expect("Impossible to start preferred rollup");
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    let mut tx_builder =
        TxBuilder::new(TxBuilderConfig { modules, validity }, &rollup.client).await;
    let mut slots = rollup.api_client.subscribe_slots().await?;

    // DA service of "second", non-preferred sequencer.
    let second_da_service =
        StorableMockDaService::new_manual_producing(regular_rollup_da_address, da_layer.clone());

    // Registering other sequencers.
    let seq_users = vec![&second_seq_user, &third_seq_user];
    let registration_call_messages = vec![
        sov_sequencer_registry::CallMessage::<S>::Register {
            da_address: regular_rollup_da_address,
            amount: setup.genesis_config.sequencer_registry.seq_bond + 1000,
        },
        sov_sequencer_registry::CallMessage::<S>::Register {
            da_address: third_rollup_da_address,
            amount: setup.genesis_config.sequencer_registry.seq_bond + 1000,
        },
    ];
    let registrations_batch: Vec<_> = registration_call_messages
        .into_iter()
        .zip(seq_users.into_iter())
        .map(|(msg, user)| {
            let TransactionType::Plain {
                message,
                key,
                details,
            } = user.create_plain_message::<RT, sov_sequencer_registry::SequencerRegistry<S>>(msg)
            else {
                panic!("`create_plain_message` haven't returned TransactionType::Plain!");
            };
            TransactionType::<RT, S>::sign(
                message,
                key,
                &RT::CHAIN_HASH,
                details,
                &mut tx_builder.nonces,
            )
        })
        .collect();

    let registration_batch_txs = registrations_batch.len();
    let register_batch_receipt = tx_builder
        .publish_transactions(registrations_batch, &rollup.client)
        .await?;
    assert_eq!(
        registration_batch_txs,
        register_batch_receipt.tx_hashes.len(),
        "not all sequencer registrations were applied"
    );
    wait_for_non_preferred_blobs_execution(
        da_layer.clone(),
        blocks_till_result,
        DEFAULT_BLOCK_TIME,
        &mut slots,
    )
    .await;

    timeout(DEFAULT_TIMEOUT, tx_builder.wait_for_results())
        .await
        .expect("Timed out while waiting for transactions to finish executing")
        .expect("Some transactions where not correctly executed!");

    let mut outputs = Vec::new();
    for _i in 0..main_loop_iterations {
        let (preferred_outputs, _) = tx_builder
            .produce_and_publish_batch(transactions_per_batch, &mut generator, &rollup.client, u)
            .await?;
        outputs.extend(preferred_outputs);
        let (regular_outputs, _) = tx_builder
            .produce_and_publish_batch_directly(
                transactions_per_batch,
                &mut generator,
                &second_da_service,
                u,
            )
            .await?;
        outputs.extend(regular_outputs);

        wait_for_non_preferred_blobs_execution(
            da_layer.clone(),
            blocks_till_result,
            DEFAULT_BLOCK_TIME,
            &mut slots,
        )
        .await;
        timeout(DEFAULT_TIMEOUT, tx_builder.wait_for_results())
            .await
            .expect("Timed out while waiting for transactions to finish executing")
            .expect("Some transactions where not correctly executed!");
    }

    let config = BasicClientConfig {
        url: rollup.client.base_url,
        rollup_height: None,
    };

    let changes = outputs
        .into_iter()
        .flat_map(|output| output.outcome.unwrap_changes())
        .collect();

    assert_logs_against_state(changes, Arc::new(config), 1)
        .await
        .expect("Failed to assert against state");

    Ok(())
}

async fn wait_for_non_preferred_blobs_execution(
    da_layer: Arc<tokio::sync::RwLock<StorableMockDaLayer>>,
    blocks: u32,
    block_time: Duration,
    slots: &mut BoxStream<'_, anyhow::Result<Slot>>,
) {
    for i in 0..blocks {
        {
            let mut da = da_layer.write().await;
            da.produce_block().await.expect("Failed to produce block");
        }
        tokio::time::sleep(block_time).await;
        let slot = timeout(DEFAULT_TIMEOUT, slots.next())
            .await
            .with_context(|| format!("Waited {i} iteration of waiting  "))
            .expect("Timed out waiting for slot to be processed")
            .expect("Error receiving slot notification")
            .expect("Empty slot notification");
        tracing::info!(n = slot.number, "Processed slot number {}", slot.number);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_half_from_non_preferred() {
    test_several_sequencers(
        Distribution::with_values(vec![(10, ModulesToUse::ValueSetter)]),
        Distribution::with_equiprobable_values(vec![MessageValidity::Valid]),
        DEFAULT_TXS_PER_BATCH,
    )
    .await
    .unwrap();
}

/// ## TODO(@theochap):
/// This test fails if the number of transactions per batch is greater than 14.
/// This is caused by nonce issues in the sequencer submission process.
///
#[tokio::test(flavor = "multi_thread")]
async fn simple_sequencer_generation_with_da() {
    let (rollup, outputs) = test_with_modules(
        Distribution::with_values(vec![
            (8, ModulesToUse::Bank),
            (2, ModulesToUse::ValueSetter),
        ]),
        Distribution::with_equiprobable_values(vec![MessageValidity::Valid]),
    )
    .await;

    let config = BasicClientConfig {
        url: rollup.client.base_url,
        rollup_height: None,
    };

    let changes = outputs
        .into_iter()
        .flat_map(|output| output.outcome.unwrap_changes())
        .collect();

    assert_logs_against_state(changes, Arc::new(config), 1)
        .await
        .expect("Failed to assert against state");
}
