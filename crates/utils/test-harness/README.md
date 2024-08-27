# Testing harness

Crate that can produce flow transactions to be pointed against running rollup.

__Note:__ If you omit `--max-num-txs` the harness will run continuously.
__Note:__ If you omit `--interval` the harness will submit transactions as fast as possible whilst adhering to the configured `--max-batch-size-txs`.

### Current limitations

 * Only Celestia DA
 * Only single sequencer, and binary submits batches directly to DA
 * Only checks that all transactions have been succeeded not change in state
 * Interval is not granular at the module level

### To Do
:black_square_button: Add something to the module message generators that can be used to query the expected state change after successful tx broadcasting