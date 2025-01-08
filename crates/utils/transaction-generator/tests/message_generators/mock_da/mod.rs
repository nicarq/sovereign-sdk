//! Uses the transaction generator to build transactions using the MockDA

use std::collections::HashMap;
use std::num::NonZero;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, TryStreamExt};
use progenitor_client::ResponseValue;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::prelude::arbitrary::Unstructured;
use sov_modules_api::prelude::tokio;
use sov_modules_api::prelude::tokio::time::timeout;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{CryptoSpec, HexHash, Runtime, Spec};
use sov_node_client::NodeClient;
use sov_rollup_interface::node::ledger_api::IncludeChildren;
use sov_sequencer::batch_builders::standard::StdBatchBuilderConfig;
use sov_sequencer::BatchBuilderConfig;
use sov_test_utils::ledger_db::sov_api_spec::types::{
    AcceptTxResponse, Slot, SubmitBatchReceipt, TxReceiptResult,
};
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, TestRollup};
use sov_test_utils::{RtAgnosticBlueprint, TestSpec as S, TransactionType};
use sov_transaction_generator::generators::basic::BasicClientConfig;
use sov_transaction_generator::interface::rng_utils::get_random_bytes;
use sov_transaction_generator::interface::{MessageValidity, Percent};
use sov_transaction_generator::{assert_logs_against_state, Distribution};

use crate::{
    plain_tx_with_default_details, setup_harness, setup_roles_and_config, GeneratorOutput,
    ModulesToUse, TestGenerator, MAX_VEC_LEN_VALUE_SETTER, RT, USER_BALANCE,
};

/// The number of transactions to generate.
pub const DEFAULT_BLOCK_PRODUCING_CONFIG: BlockProducingConfig = BlockProducingConfig::Periodic;
pub const DEFAULT_BLOCK_TIME_MS: u64 = 150;
pub const DEFAULT_FINALIZATION_BLOCKS: u32 = 5;
pub const DEFAULT_TXS_PER_BATCH: u64 = 10;
pub const DEFAULT_TIMEOUT: Duration = Duration::new(10, 0);

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
    fn sign_generator_output(&mut self, gen_output: GeneratorOutput) -> Transaction<RT, S> {
        let TransactionType::Plain {
            message,
            key,
            details,
        } = plain_tx_with_default_details(&gen_output)
        else {
            panic!("The method `plain_tx_with_default_details` should return a plain transaction!");
        };

        TransactionType::<RT, S>::sign(message, key, &RT::CHAIN_HASH, details, &mut self.nonces)
    }

    async fn produce_and_publish_batch(
        &mut self,
        num_txs_in_batch: u64,
        generator: &mut TestGenerator,
        client: &NodeClient,
        u: &mut Unstructured<'_>,
    ) -> Result<(Vec<GeneratorOutput>, SubmitBatchReceipt), anyhow::Error> {
        let modules = self.config.modules.clone().map_values(&mut |module| {
            module.select(&generator.bank_harness, &generator.value_setter_harness)
        });

        let generator_outputs: Vec<_> = (0..num_txs_in_batch)
            .map(|_| {
                let validity = self
                    .config
                    .validity
                    .select_value(u)
                    .expect("Ran out of randomness");

                generator.generate(&modules, *validity)
            })
            .collect();

        let txs: Vec<_> = generator_outputs
            .iter()
            .map(|output| self.sign_generator_output(output.clone()))
            .collect();

        let batch_result = client
            .client
            .publish_batch_with_serialized_txs(&txs)
            .await?;

        for tx_hash in &batch_result.tx_hashes {
            let tx_hash = tx_hash.parse().expect("Invalid tx hash");
            self.txs_to_wait_for.push(tx_hash);
        }

        Ok((generator_outputs, batch_result))
    }

    async fn sign_and_send_generator_output(
        &mut self,
        gen_output: GeneratorOutput,
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
    // We are sending transactions with the basic batch builder here.
    // To use the preferred batch builder, one should use a runtime that implements the soft-confirmation
    // kernel. We can easily generate it with the testing framework.
    .set_config(|config| {
        config.batch_builder_config = BatchBuilderConfig::Standard(StdBatchBuilderConfig {
            mempool_max_txs_count: NonZero::new((DEFAULT_TXS_PER_BATCH * 2) as usize),
            max_batch_size_bytes: None,
        });
        config.prover_address = setup.attester.user_info.address().to_string();
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
            .sign_and_send_generator_output(init_tx, &rollup.client)
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
