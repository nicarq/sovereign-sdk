use std::sync::Arc;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use futures::StreamExt;
use sov_api_spec::types::AcceptTxBody;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::{BlockProducingConfig, MockAddress};
use sov_modules_api::{RawTx, Runtime};
use sov_modules_stf_blueprint::GenesisParams;
use sov_rollup_interface::common::SafeVec;
use sov_rollup_interface::da::BlobReaderTrait;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::TxHash;
use sov_stf_runner::processes::RollupProverConfig;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder};
use sov_test_utils::{
    default_test_signed_transaction, generate_optimistic_runtime, RtAgnosticBlueprint, TestSpec,
    TestUser,
};

generate_optimistic_runtime!(TestRuntime <=);

type TestBlueprint = RtAgnosticBlueprint<TestSpec, TestRuntime<TestSpec>>;

#[derive(Debug, Clone)]
struct IterationCase {
    via_accept_tx: usize,
    via_submit: usize,
}

#[tokio::test(flavor = "multi_thread")]
async fn test_thin_direct_same_transactions() -> anyhow::Result<()> {
    // Test starts a rollup and thin direct sequencer.
    // It submits the same transactions to both and checks that:
    //  1. Thin sequencer returns the same tx_hashes-the
    //  2. The same blob is posted to DA.
    let dir1 = Arc::new(tempfile::tempdir()?);

    let genesis_config: HighLevelOptimisticGenesisConfig<TestSpec> =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let genesis_conf_seq_da_address = genesis_config.initial_sequencer.da_address;
    let mut genesis_params = GenesisParams {
        runtime: <TestRuntime<TestSpec> as Runtime<TestSpec>>::GenesisConfig::from_minimal_config(
            genesis_config.clone().into(),
        ),
    };
    genesis_params
        .runtime
        .sequencer_registry
        .is_preferred_sequencer = false;
    let test_rollup = RollupBuilder::<TestBlueprint>::new(
        GenesisSource::CustomParams(genesis_params),
        BlockProducingConfig::Manual,
        1,
        0,
        Default::default(),
    )
    .set_config(|c| {
        c.storage = dir1;
        c.rollup_prover_config = RollupProverConfig::Skip;
    })
    .set_da_config(|c| {
        c.sender_address = genesis_conf_seq_da_address;
    })
    .with_standard_batch_builder()
    .with_secondary_sequencer(MockAddress::new([128; 32]))
    .start()
    .await?;

    let test_sequencer_client = test_rollup
        .secondary_test_sequencer_client
        .as_ref()
        .unwrap();

    let head = test_rollup.da_service.get_head_block_header().await?.height;
    let mut slots = test_rollup.api_client.subscribe_slots().await?;

    let user = genesis_config.additional_accounts.first().unwrap();
    let mut all_txs = generate_txs(user, 8);

    let cases = vec![
        // 1. submit 3 txs in `publishBatch` only
        IterationCase {
            via_accept_tx: 0,
            via_submit: 3,
        },
        // 2. accept 3 txs, then empty `publishBatch`
        IterationCase {
            via_accept_tx: 3,
            via_submit: 0,
        },
        // 3. accept 1 txs, then submit 1 tx in `publishBatch`
        IterationCase {
            via_accept_tx: 1,
            via_submit: 1,
        },
    ];

    for (idx, case) in cases.into_iter().enumerate() {
        let IterationCase {
            via_accept_tx,
            via_submit,
        } = case.clone();
        let expected_total_hashes = via_accept_tx + via_submit;

        let via_accept_txs = all_txs.drain(..via_accept_tx).collect::<Vec<_>>();
        // Accept first
        for tx in via_accept_txs {
            let tx_hash_accepted_a = accept_tx_in_rollup(&test_rollup.api_client, &tx).await?;
            let tx_hash_accepted_b = accept_tx_in_rollup(test_sequencer_client, &tx).await?;
            assert_eq!(tx_hash_accepted_a, tx_hash_accepted_b);
        }

        // Submit batch
        let via_submit_txs = all_txs.drain(..via_submit).collect::<Vec<_>>();
        let tx_hashes_a = publish_batch_in_rollup(&test_rollup.api_client, &via_submit_txs).await?;
        let tx_hashes_b = publish_batch_in_rollup(test_sequencer_client, &via_submit_txs).await?;
        assert_eq!(
            expected_total_hashes,
            tx_hashes_a.len(),
            "Wrong number total hashes returned from StandardSequencer for case: {:?}",
            case
        );
        assert_eq!(tx_hashes_a, tx_hashes_b, "Number of hashes mismatch between StandardSequencer(left) and TestStatelessSequencer(right) for case {:?}", case);

        test_rollup.da_service.produce_block_now().await?;

        // Wait for the slot to be processed, so rollup is in a good state.
        let _slot = slots.next().await.unwrap()?;

        // Compare submitted data
        compare_block_at_height(head + 1 + idx as u64, &test_rollup.da_service).await;
    }
    Ok(())
}

fn generate_txs(user: &TestUser<TestSpec>, number: u64) -> Vec<RawTx> {
    (1..=number)
        .map(|i| {
            let msg = TestRuntimeCall::Bank(
                sov_test_utils::sov_bank::CallMessage::<TestSpec>::CreateToken {
                    token_name: format!("sequencers-check-{}", i).try_into().unwrap(),
                    initial_balance: 1000,
                    mint_to_address: user.address(),
                    admins: SafeVec::new(),
                },
            );

            let tx = default_test_signed_transaction::<TestRuntime<TestSpec>, TestSpec>(
                &user.private_key,
                &msg,
                i,
                &TestRuntime::<TestSpec>::CHAIN_HASH,
            );

            RawTx::new(borsh::to_vec(&tx).unwrap())
        })
        .collect()
}

async fn publish_batch_in_rollup(
    api_client: &sov_api_spec::client::Client,
    txs: &[RawTx],
) -> anyhow::Result<Vec<TxHash>> {
    let request_body = sov_api_spec::types::PublishBatchBody {
        transactions: txs
            .iter()
            .map(|tx| BASE64_STANDARD.encode(&tx.data))
            .collect(),
    };

    let response = api_client.publish_batch(&request_body).await?;

    let receipt = response
        .data
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Empty publish batch receipt data was empty"))?;

    assert!(receipt.tx_hashes.len() >= txs.len());

    Ok(receipt
        .tx_hashes
        .into_iter()
        .map(|t| t.parse().unwrap())
        .collect())
}

async fn accept_tx_in_rollup(
    api_client: &sov_api_spec::client::Client,
    tx: &RawTx,
) -> anyhow::Result<TxHash> {
    let accept_tx_body = AcceptTxBody {
        body: BASE64_STANDARD.encode(&tx.data),
    };
    let tx_accepted = api_client.accept_tx(&accept_tx_body).await?;
    tx_accepted.data.id.parse()
}

async fn compare_block_at_height(height: u64, da_service_1: &StorableMockDaService) {
    let mut block = da_service_1
        .get_block_at(height)
        .await
        .expect("Failed to get block from DaService1");
    assert_eq!(block.batch_blobs.len(), 2);
    let raw_blobs: Vec<_> = block
        .batch_blobs
        .iter_mut()
        .map(|b| b.full_data().to_vec())
        .collect();
    assert!(!raw_blobs[0].is_empty());
    assert_eq!(raw_blobs[0], raw_blobs[1]);
}
