//! Integration tests for the standard sequencer that use [`TestSequencerSetup`].
//!
//! DEPRECATED. See <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1881>.

use base64::prelude::*;
use sov_api_spec::types::{self};
use sov_mock_da::MockDaService;
use sov_sequencer::batch_builders::preferred::PreferredBatchBuilder;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::sequencer::TestSequencerSetup;
use sov_test_utils::TestSpec;

use crate::preferred_end_to_end::TestRuntime;
use crate::utils::generate_paymaster_tx;

// Checks that transactions that are not sequencer safe are rejected
// when the sender address is not configured as an admin in the sequencer config.
#[tokio::test(flavor = "multi_thread")]
async fn not_sequencer_safe_txs_are_restricted() {
    let dir = tempfile::tempdir().unwrap();
    let sequencer_addr = HighLevelOptimisticGenesisConfig::<TestSpec>::sequencer_da_addr();
    let da_service = MockDaService::new(sequencer_addr);

    let sequencer =
        TestSequencerSetup::<PreferredBatchBuilder<(TestSpec, TestRuntime<TestSpec>)>>::new(
            dir,
            da_service,
            Default::default(),
            false,
        )
        .await
        .unwrap();

    let tx = generate_paymaster_tx(sequencer.admin_private_key.clone());
    {
        let client = sequencer.client();

        if let Err(e) = client
            .accept_tx(&types::AcceptTxBody {
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

// Checks that transactions that are not sequencer safe are accepted
// if the sender address is configured as an admin in the sequencer config.
#[tokio::test(flavor = "multi_thread")]
async fn sequencer_safe_txs_from_admins_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let sequencer_addr = HighLevelOptimisticGenesisConfig::<TestSpec>::sequencer_da_addr();
    let da_service = MockDaService::new(sequencer_addr);

    let sequencer =
        TestSequencerSetup::<PreferredBatchBuilder<(TestSpec, TestRuntime<TestSpec>)>>::new(
            dir,
            da_service,
            Default::default(),
            true,
        )
        .await
        .unwrap();

    let tx = generate_paymaster_tx(sequencer.admin_private_key.clone());
    {
        let client = sequencer.client();

        client
            .accept_tx(&types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await.expect("Transactions from sequencer admins should be accepted even if they are sequencer unsafe");
    }
}
