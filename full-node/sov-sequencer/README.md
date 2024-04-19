# Sov-Sequencer

Simple implementation of based sequencer generic over batch builder and DA service.

Exposes 2 RPC methods:

1. `sequencer_acceptTx` where input is supposed to be signed and serialized transaction. This transaction is stored in mempool
2. `sequencer_publishBatch` without any input, which builds the batch using batch builder and publishes it on DA layer.

### Submit transactions

Please see [`demo-rollup` README](../../examples/demo-rollup/README.md#how-to-submit-transactions).

### Publish blob

To submit transactions to DA layer, sequencer needs to publish them. This can be done by triggering `publishBatch` endpoint:

```bash
./target/debug/sov-cli publish-batch http://127.0.0.1:12345
```

or using plain curl:

```bash
curl -X POST http://127.0.0.1:12345 -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"sequencer_publishBatch","params":[],"id":1}'
```

After some time, processed transaction should appear in logs of running rollup
