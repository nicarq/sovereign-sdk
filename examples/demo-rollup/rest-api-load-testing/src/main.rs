use helpers::create_batch;
use rest_api_load_testing::Requests;

const PRIVATE_KEYS_FILE: &str = "../../test-data/keys/token_deployer_private_key.json";
const URL: &str = "http://localhost:12346";

#[tokio::main]
async fn main() {
    // TODO #1867
    create_batch(PRIVATE_KEYS_FILE, URL.to_string()).await;
    // To check available endpoints: http://localhost:12346/swagger-ui/

    let mut all_endpoints = Vec::new();
    all_endpoints.append(&mut ledger_endpoints());
    all_endpoints.append(&mut module_endpoints());
    all_endpoints.append(&mut rollup_endpoints());

    let requests = Requests::new(URL, all_endpoints);
    let summary = rest_api_load_testing::start(requests).await;
    summary.print_summary();
}

fn ledger_endpoints() -> Vec<&'static str> {
    // TODO: To test the commented endpoints, we need to send some transactions to the full node. #1807
    vec![
        "ledger/slots/latest",
        "ledger/batches/0",
        "ledger/batches/0/txs/0",
        "ledger/batches/0/txs/0/events/0",
        "ledger/events/0",
        //"ledger/slots/{slotId}/batches/{batchOffset}",
        // "ledger/slots/{slotId}/batches/{batchOffset}/txs/{txOffset}/events/{eventOffset}",
        // "ledger/slots/{slotId}/events",
        "ledger/txs/0",
        "ledger/txs/0/events/0",
    ]
}

fn module_endpoints() -> Vec<&'static str> {
    // TODO: To test the commented endpoints, we need to send some transactions to the full node. #1807
    vec![
        // Accounts
        "modules/accounts/state/accounts",
        // "modules/accounts/state/accounts/items/{key}",
        "modules/accounts/state/credential-ids",
        // "modules/accounts/state/credential-ids/items/{key}",
        // Bank
        "modules/bank/tokens",
        // "modules/bank/tokens/{token_id}/balances/{address}",
        // "modules/bank/tokens/{token_id}/total-supply",
        // Nonces
        "modules/nonces/state/nonces",
        // "modules/nonces/state/nonces/items/{key}",
        // Sequencer
        "modules/sequencer-registry/state/allowed-sequencers",
        //"modules/sequencer-registry/state/allowed-sequencers/items/{key}",
        "modules/sequencer-registry/state/preferred-sequencer",
        // ChainState
        "modules/chain-state/state/genesis-da-height",
        "modules/chain-state/state/inner-code-commitment",
        "modules/chain-state/state/next-visible-rollup-height",
        "modules/chain-state/state/next-visible-rollup-height",
        "modules/chain-state/state/operating-mode",
        "modules/chain-state/state/outer-code-commitment",
        "modules/chain-state/state/slots",
        // "modules/chain-state/state/slots/items/{index}",
        "modules/chain-state/state/state-roots",
        // "modules/chain-state/state/state-roots/items/{index}",
        "modules/chain-state/state/time",
        "modules/chain-state/state/true-rollup-height",
        "modules/chain-state/state/true-to-visible-rollup-height-history",
        "modules/chain-state/state/true-to-visible-rollup-height-history",
    ]
}

fn rollup_endpoints() -> Vec<&'static str> {
    // TODO: To test the commented endpoints, we need to send some transactions to the full node. #1807
    vec![
        "rollup/base-fee-per-gas/latest",
        //"rollup/addresses/{address}/dedup",
        "rollup/sync-status",
    ]
}

mod helpers {
    use std::path::Path;

    use anyhow::Context;
    use demo_stf::runtime::{Runtime, RuntimeCall};
    use sov_cli::wallet_state::PrivateKeyAndAddress;
    use sov_modules_api::transaction::Transaction;
    use sov_modules_api::{Runtime as RuntimeTrait, SafeVec};
    use sov_test_utils::{default_test_signed_transaction, TestPrivateKey, TestSpec};

    const TOKEN_NAME: &str = "TestToken";

    struct Client(sov_api_spec::Client);

    impl Client {
        fn new(api_url: String) -> Self {
            let client = sov_api_spec::Client::new(&api_url);

            Self(client)
        }

        pub async fn send_transactions(
            &self,
            transactions: &[Transaction<Runtime<TestSpec>, TestSpec>],
        ) {
            let _submitted_batch_info = self
                .0
                .publish_batch_with_serialized_txs(transactions)
                .await
                .unwrap();
        }
    }

    fn build_create_token_tx(
        key: &TestPrivateKey,
        nonce: u64,
        initial_balance: u64,
    ) -> Transaction<Runtime<TestSpec>, TestSpec> {
        let user_address = key.to_address();
        let msg = RuntimeCall::Bank(sov_bank::CallMessage::<TestSpec>::CreateToken {
            token_name: TOKEN_NAME.try_into().unwrap(),
            initial_balance,
            mint_to_address: user_address,
            authorized_minters: SafeVec::new(),
        });

        default_test_signed_transaction(key, &msg, nonce, &Runtime::<TestSpec>::CHAIN_HASH)
    }

    pub(crate) async fn create_batch(private_key_file: &str, url: String) {
        let client = Client::new(url);
        let keys = PrivateKeyAndAddress::<TestSpec>::from_json_file(Path::new(private_key_file))
            .context(format!("File does not exist: {:?}", private_key_file))
            .unwrap();
        let priv_key = keys.private_key;
        let tx = build_create_token_tx(&priv_key, 0, 100);
        client.send_transactions(&[tx]).await;
    }
}
