# Testing harness

This crate contains binary that can execute a set of transactions against [sov-demo-rollup](../../examples/demo-rollup/README_CELESTIA.md) on Celestia DA.


## Current limitations

 * Only `sov-demo-rollup` runtime
 * Only `sov-bank` transfers
 * Only Celestia DA
 * Only single sequencer, and binary submits batches directly to DA
 * Only checks that all transactions have been succeeded not change in state


## Example command:

Make sure the celestia version of the demo rollup is running in another window

From repo root.

```bash
cargo run --bin testing-harness -- \
    --private-keys-dir examples/test-data/keys \
    --genesis-dir examples/test-data/genesis/demo/celestia \
    --rollup-config-path examples/demo-rollup/demo_rollup_config.toml \
    --max-batch-size-tx=100 \
    --max-batch-size-bytes=100000 \
    --new-users-count=1000 \
    --bank-transactions-count=1000
```