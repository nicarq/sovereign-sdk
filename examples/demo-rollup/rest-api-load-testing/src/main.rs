use rest_api_load_testing::Requests;

#[tokio::main]
async fn main() {
    //To check available endpoints: http://localhost:12346/swagger-ui/

    let mut all_endpoints = Vec::new();
    all_endpoints.append(&mut ledger_endpoints());
    all_endpoints.append(&mut module_endpoints());
    all_endpoints.append(&mut rollup_endpoints());

    let requests = Requests::new("http://localhost:12346", all_endpoints);
    let summary = rest_api_load_testing::start(requests).await;
    summary.print_summary();
}

fn ledger_endpoints() -> Vec<&'static str> {
    // TODO: To test the commented endpoints, we need to send some transactions to the full node. #1807
    vec![
        "ledger/slots/latest",
        // "ledger/aggregated-proofs/latest",
        // "ledger/batches/{batch_id}/transactions",
        // "ledger/batches/{batchId}",
        // "ledger/batches/{batchId}/txs/{txOffset}",
        // "ledger/batches/{batchId}/txs/{txOffset}/events/{eventOffset}",
        // "ledger/events/{eventId}",
        // "ledger/slots/{slotId}/batches/{batchOffset}",
        // "ledger/slots/{slotId}/batches/{batchOffset}/txs/{txOffset}/events/{eventOffset}",
        // "ledger/slots/{slotId}/events",
        // "ledger/txs/{txId}",
        // "ledger/txs/{txId}/events/{eventOffset}",
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
