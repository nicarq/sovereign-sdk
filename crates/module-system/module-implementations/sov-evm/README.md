# `sov-evm` module

The sov-evm module provides compatibility with the EVM.

The module `CallMessage` contains `rlp` encoded Ethereum transaction, which is validated & executed immediately after being dispatched from the DA. Once all transactions from the DA slot have been processed, they are grouped into an `Ethereum` block. Users can access information such as receipts, blocks, transactions, and more through standard Ethereum endpoints.


## Genesis

### Accounts

Each EVM account, specified in `data[].address` need to have default credential ID in `accounts.json` genesis and potentially some funds in `bank.json`.

For example, parts of `evm.json`:

```json
{
  "data": [
    {
      "address": "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
      "balance": "0xffffffffffffffff",
      "code_hash": "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470",
      "code": "0x",
      "nonce": 0
    }
  ]
}
```

It needs to have some rollup address generated and placed in `address.json` 
```json
{
  "accounts": [
    {
      "credential_id": "0x000000000000000000000000f39fd6e51aad88f6f4ce6ab8827279cfffb92266",
      "address": "sov1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5z5tpwxqergd3crhxalf"
    }
  ]
}
```

and then 

## Note to developers (hooks.rs)

WARNING: `prevrandao` value is predictable up to `DEFERRED_SLOTS_COUNT` in advance,
Users should follow the same best practice that they would on Ethereum and use future randomness.
See: `<https://eips.ethereum.org/EIPS/eip-4399#tips-for-application-developers>`

## Integration Details

The EVM provides has its own nonce mechanism that's less flexible than the one provided by the SDK. Since we have native account abstraction, we simulate the EVM nonce handling 
by tracking one nonce per EVM *account* in the EVM module. Separately, we track a deduplicator (nonce and/or generation number) per *credential* in the SDK. When accepting EVM transactions,
we use the Sovereign SDK's native deduplicator to check the transaction. Once the tx has passed all pre-flight checks, we silently overwrite the nonce field seen by the EVM with the 
globally unique `nonce` tracked by the EVM module. This means that you can safely use the SDK's native account abstraction to have multiple private keys control your account using
separate nonces, and the EVM will still behave as expected (i.e. any repeated calls to the `CREATE` opcode will always generate unique addresses).
