This crate contains binary that can execute a set of transactions against [sov-demo-rollup](../../README_CELESTIA.md) on
Celestia DA.

### Example command:

Make sure the celestia version of the demo rollup is running in another terminal.

Then, from the repo root, to run:

```bash
cargo run --bin sov-demo-celestia-harness -- \
    --private-keys-dir examples/test-data/keys \
    --rollup-config-path examples/demo-rollup/demo_rollup_config.toml \
    --max-batch-size-tx=5 \
    --max-batch-size-bytes=10000 \
    --new-users-count=10 \
    --max-num-txs=50
```