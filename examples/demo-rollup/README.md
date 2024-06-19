# Demo Rollup ![Time - ~5 mins](https://img.shields.io/badge/Time-~5_mins-informational)

<p align="center">
  <img width="50%" src="../../assets/discord-banner.png">
  <br>
  <i>Stuck, facing problems, or unsure about something?</i>
  <br>
  <i>Join our <a href="https://discord.gg/kbykCcPrcA">Discord</a> and ask your questions in <code>#support</code>!</i>
</p>

#### Table of Contents

<!-- https://github.com/thlorenz/doctoc -->
<!-- $ doctoc README.md --github --notitle -->
<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [What is This?](#what-is-this)
- [Getting Started](#getting-started)
  - [Run a local DA layer instance](#run-a-local-da-layer-instance)
  - [Start the Rollup Full Node](#start-the-rollup-full-node)
  - [Sanity Check: Creating a Token](#sanity-check-creating-a-token)
  - [How to Submit Transactions](#how-to-submit-transactions)
    - [1. Build `sov-cli`](#1-build-sov-cli)
    - [2. Generate the Transaction](#2-generate-the-transaction)
    - [Submit the Transaction(s)](#submit-the-transactions)
    - [Verify the Token Supply](#verify-the-token-supply)
- [Disclaimer](#disclaimer)
- [Interacting with your Node via RPC](#interacting-with-your-node-via-rpc)
  - [Key Concepts](#key-concepts)
  - [RPC Methods](#rpc-methods)
    - [`ledger_getHead`](#ledger_gethead)
    - [`ledger_getSlots`](#ledger_getslots)
    - [`ledger_getBatches`](#ledger_getbatches)
    - [`ledger_getTransactions`](#ledger_gettransactions)
    - [`ledger_getEvents`](#ledger_getevents)
- [Testing with specific DA layers](#testing-with-specific-da-layers)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## What is This?

This demo shows how to integrate a State Transition Function (STF) with a Data Availability (DA) layer and a zkVM to
create a full
zk-rollup. The code in this repository corresponds to running a full-node of the rollup, which executes
every transaction.

By swapping out or modifying the imported state transition function, you can customize
this example full-node to run arbitrary logic.
This particular example relies on the state transition exported by [`demo-stf`](../demo-rollup/stf/). If you want to
understand how to build your own state transition function, check out at the docs in that package.

## Getting Started

If you are looking for a simple rollup with minimal dependencies as a starting point, please have a look here:
[sov-rollup-starter](https://github.com/Sovereign-Labs/sov-rollup-starter/)

If you don't need ZK guest to be compiled, for faster compilation time you can export `export SKIP_GUEST_BUILD=1`
environment
variable in each terminal you run. By default, demo-rollup disables proving. If you want to enable proving, several options
are available:

- `export SOV_PROVER_MODE=skip` Skips verification logic.
- `export SOV_PROVER_MODE=simulate` Run the rollup verification logic inside the current process.
- `export SOV_PROVER_MODE=execute` Run the rollup verifier in a zkVM executor.
- `export SOV_PROVER_MODE=prove` Run the rollup verifier and create a SNARK of execution.

### Run a local DA layer instance

This setup works with an in-memory DA that is easy to set up for testing purposes.

### Start the Rollup Full Node

1. Switch to the `examples/demo-rollup` and compile the application:

```shell,test-ci
$ cd examples/demo-rollup/
$ make build
```

2. Clean up the existing database.
   Makefile to simplify that process:

```sh,test-ci
$ make clean
```

3. Now run the demo-rollup full node, as shown below.

```sh,test-ci
$ export SOV_PROVER_MODE=execute
```

```sh,test-ci,bashtestmd:long-running,bashtestmd:wait-until=rpc_address
$ cargo run
```

Leave it running while you proceed with the rest of the demo.

### Sanity Check: Creating a Token

After switching to a new terminal tab, let's submit our first transaction by creating a token:

```sh,test-ci
$ make test-create-token
```

Once a batch is submitted the output should also contain the transaction hashes that have been submitted. For example -

```text
Your batch was submitted to the sequencer for publication. Response: "Submitted 1 transactions"
0: 66d4a27dd46013f88c156d21d16d364f6a5de66effd74155a5b0815475cbdf17
```

The transaction hash can be used to query the RPC endpoint to fetch events belonging to the transaction, which should in
this case have the TokenCreated Event

```sh,test-ci
$ curl -sS -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getEventsByTxnHash","params":["66d4a27dd46013f88c156d21d16d364f6a5de66effd74155a5b0815475cbdf17"],"id":1}' http://127.0.0.1:12345
{"jsonrpc":"2.0","result":[{"event_value":{"TokenCreated":{"token_id":"token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6"}},"module_name":"bank","module_id":"module_1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sh0zjpk"}],"id":1}%
```

We can see the TokenCreated event which contains the id of the token
created - `token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6`

### How to Submit Transactions

The `make test-create-token` command above was useful to test if everything is running correctly. Now let's get a better
understanding of how to create and submit a transaction.

#### 1. Build `sov-cli`

You'll need the `sov-cli` binary in order to create transactions. Build it with these commands:

```bash,test-ci,bashtestmd:compare-output
# Make sure you're still in `examples/demo-rollup`
$ SKIP_GUEST_BUILD=1 cargo build --bin sov-cli
$ ./../../target/debug/sov-cli --help
Usage: sov-cli <COMMAND>

Commands:
  transactions  Generate, sign, list and remove transactions
  keys          View and manage keys associated with this wallet
  rpc           Query the current state of the rollup and send transactions
  help          Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

Each transaction that we want to submit is a member of the `CallMessage` enum defined as part of creating a module. For
example, let's consider the `Bank` module's `CallMessage`:

```rust
use sov_bank::CallMessage::Transfer;
use sov_bank::Coins;
use sov_bank::TokenId;
use sov_bank::Amount;

pub enum CallMessage<S: sov_modules_api::Spec> {
    /// Creates a new token with the specified name and initial balance.
    CreateToken {
        /// Random value used to create a unique token ID.
        salt: u64,
        /// The name of the new token.
        token_name: String,
        /// The initial balance of the new token.
        initial_balance: Amount,
        /// The address of the account that the new tokens are minted to.
        mint_to_address: S::Address,
        /// Authorized minter list.
        authorized_minters: Vec<S::Address>,
    },

    /// Transfers a specified amount of tokens to the specified address.
    Transfer {
        /// The address to which the tokens will be transferred.
        to: S::Address,
        /// The amount of tokens to transfer.
        coins: Coins,
    },

    /// Burns a specified amount of tokens.
    Burn {
        /// The amount of tokens to burn.
        coins: Coins,
    },

    /// Mints a specified amount of tokens.
    Mint {
        /// The amount of tokens to mint.
        coins: Coins,
        /// Address to mint tokens to
        mint_to_address: S::Address,
    },

    /// Freeze a token so that the supply is frozen
    Freeze {
        /// Address of the token to be frozen
        token_id: TokenId,
    },
}
```

In the above snippet, we can see that `CallMessage` in `Bank` supports five different types of calls. The `sov-cli` has
the ability to parse a JSON file that aligns with any of these calls and subsequently serialize them. The structure of
the JSON file, which represents the call, closely mirrors that of the Enum member. You can view the relevant JSON Schema
for `Bank` [here](../../module-system/module-schemas/schemas/sov-bank.json) Consider the `Transfer` message as an
example:

```rust
use sov_bank::Coins;

struct Transfer<S: sov_modules_api::Spec> {
    /// The address to which the tokens will be transferred.
    to: S::Address,
    /// The amount of tokens to transfer.
    coins: Coins,
}
```

Here's an example of a JSON representing the above call:

```json
{
  "Transfer": {
    "to": "sov1zgfpyysjzgfpyysjzgfpyysjzgfpyysjzgfpyysjzgfpyysjzgfqve8h6h",
    "coins": {
      "amount": 200,
      "token_id": "token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6"
    }
  }
}
```

#### 2. Generate the Transaction

The JSON above is the contents of the
file [`examples/test-data/requests/transfer.json`](../../examples/test-data/requests/transfer.json). We'll use this
transaction as our example for the rest of the tutorial. In order to send the transaction, we need to perform 2
operations:

- Import the transaction data into the wallet
- Sign and submit the transaction

Note: we're able to make a `Transfer` call here because we already created the token as part of the sanity check above,
using `make test-create-token`.

To generate transactions you can use the `transactions import from-file` subcommand, as shown below:

```bash,test-ci,bashtestmd:compare-output
$ ./../../target/debug/sov-cli transactions import from-file -h
Import a transaction from a JSON file at the provided path

Usage: sov-cli transactions import from-file <COMMAND>

Commands:
  bank                A subcommand for the `bank` module
  sequencer-registry  A subcommand for the `sequencer_registry` module
  value-setter        A subcommand for the `value_setter` module
  prover-incentives   A subcommand for the `prover_incentives` module
  accounts            A subcommand for the `accounts` module
  nonces              A subcommand for the `nonces` module
  nft                 A subcommand for the `nft` module
  help                Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
```

Let's go ahead and import the transaction into the wallet

```bash,test-ci,bashtestmd:compare-output
$ ./../../target/debug/sov-cli transactions import from-file bank --max-fee 1000000 --path ../test-data/requests/transfer.json
Adding the following transaction to batch:
{
  "tx": {
    "bank": {
      "Transfer": {
        "to": "sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94",
        "coins": {
          "amount": 200,
          "token_id": "token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6"
        }
      }
    }
  },
  "chain_id": 4321,
  "max_priority_fee_bips": 0,
  "max_fee": 1000000,
  "gas_limit": null
}
```

#### Submit the Transaction(s)

You now have a batch with a single transaction in your wallet. If you want to submit any more transactions as part of
this
batch, you can import them now. Finally, let's submit your transaction to the rollup.

```bash,test-ci
$ sleep 20  # Wait a bit for the `make test-create-token` transaction to be processed.
$ ./../../target/debug/sov-cli rpc submit-batch by-address sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94
```

#### Verify the Token Supply

```bash,test-ci,bashtestmd:compare-output
$ sleep 20  # Wait a bit for the block to be processed by the node
$ curl -sS -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"bank_supplyOf","params":{"token_id":"token_1zdwj8thgev2u3yyrrlekmvtsz4av4tp3m7dm5mx5peejnesga27ss0lusz"},"id":1}' http://127.0.0.1:12345
{"jsonrpc":"2.0","result":{"amount":1000000},"id":1}
```

```bash,test-ci,bashtestmd:compare-output
$ curl --silent -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getAggregatedProof","id":1}' http://127.0.0.1:12345 | jq '.result.proof.public_data.initial_slot_number'
1
```

## Disclaimer

> ⚠️ Warning! ⚠️

`demo-rollup` is a prototype! It contains known vulnerabilities and should not be used in production under any
circumstances.

## Interacting with your Node via RPC

By default, this implementation prints the state root and the number of blobs processed for each slot. To access any
other data, you'll
want to use our RPC server. You can configure its host and port in `rollup_config.toml`.

### Key Concepts

**Query Modes**

Most queries for ledger information accept an optional `QueryMode` argument. There are three QueryModes:

- `Standard`. In Standard mode, a response to a query for an outer struct will contain the full outer struct and hashes
  of inner structs. For example
  a standard `ledger_getSlots` query would return all information relating to the requested slot, but only the hashes of
  the batches contained therein.
  If no `QueryMode` is specified, a `Standard` response will be returned
- `Compact`. In Compact mode, even the hashes of child structs are omitted.
- `Full`. In Full mode, child structs are recursively expanded. So, for example, a query for a slot would return the
  slot's data, as well as data relating
  to any `batches` that occurred in that slot, any transactions in those batches, and any events that were emitted by
  those transactions.

**Identifiers**

There are several ways to uniquely identify items in the ledger DB.

- By _number_. Each family of structs (`slots`, `blocks`, `transactions`, and `events`) is numbered in order starting
  from `1`. So, for example, the
  first transaction to appear on the DA layer will be numered `1` and might emit events `1`-`5`. Or, slot `17` might
  contain batches `41` - `44`.
- By _hash_. (`slots`, `blocks`, and `transactions` only)
- By _containing item_id and offset_.
- (`Events` only) By _transaction_id and key_.

To request an item from the ledger DB, you can provide any identifier - and even mix and match different identifiers. We
recommend using item number
wherever possible, though, since resolving other identifiers may require additional database lookups.

Some examples will make this clearer. Suppose that slot number `5` contains batches `9`, `10`, and `11`, that batch `10`
contains
transactions `50`-`81`, and that transaction `52` emits event number `17`. If we want to fetch events number `17`, we
can use any of the following queries:

- `{"jsonrpc":"2.0","method":"ledger_getEvents","params":[[17]], ... }`
- `{"jsonrpc":"2.0","method":"ledger_getEvents","params":[[{"transaction_id": 50, "offset": 0}]], ... }`
- `{"jsonrpc":"2.0","method":"ledger_getEvents","params":[[{"transaction_id": 50, "key": [1, 2, 4, 2, ...]}]], ... }`
- `{"jsonrpc":"2.0","method":"ledger_getEvents","params":[[{"transaction_id": { "batch_id": 10, "offset": 2}, "offset": 0}]], ... }`

### RPC Methods

#### `ledger_getHead`

This method returns the current head of the ledger. It has no arguments.

**Example Query:**

```shell
$ curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getHead","params":[],"id":1}' http://127.0.0.1:12345

{"jsonrpc":"2.0","result":{"number":22019,"hash":"0xe8daef0f58a558aea44632a420bb62318bff6c38bbc616ff849d0a4be0a69cd3","batch_range":{"start":2,"end":2}},"id":1}
```

This response indicates that the most recent slot processed was number `22019`, its hash, and that it contained no
batches (since the `start` and `end`
of the `batch_range` overlap). It also indicates that the next available batch to occur will be numbered `2`.

#### `ledger_getSlots`

This method retrieves slot data. It takes two arguments, a list of `SlotIdentifier`s and an optional `QueryMode`. If no
query mode is provided,
this list of identifiers may be flattened: `"params":[[7]]` and `"params":[7]` are both acceptable,
but `"params":[7, "Compact"]` is not.

**Example Query:**

```shell
$ curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getSlots","params":[[7], "Compact"],"id":1}' http://127.0.0.1:12345

{"jsonrpc":"2.0","result":[{"number":6,"hash":"0x6a23ea92fbe3250e081b3e4c316fe52bda53d0113f9e7f8f495afa0e24b693ff","batch_range":{"start":1,"end":2}}],"id":1}
```

This response indicates that slot number `6` contained batch `1` and gives the

#### `ledger_getBatches`

This method retrieves slot data. It takes two arguments, a list of `BatchIdentifier`s and an optional `QueryMode`. If no
query mode is provided,
this list of identifiers may be flattened: `"params":[[7]]` and `"params":[7]` are both acceptable,
but `"params":[7, "Compact"]` is not.

**Example Query:**

```shell
$ curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getBatches","params":[["0xf784a42555ed652ed045cc8675f5bc11750f1c7fb0fbc8d6a04470a88c7e1b6c"]],"id":1}' http://127.0.0.1:12345

{"jsonrpc":"2.0","result":[{"hash":"0xf784a42555ed652ed045cc8675f5bc11750f1c7fb0fbc8d6a04470a88c7e1b6c","tx_range":{"start":1,"end":2},"txs":["0x191d87a51e4e1dd13b4d89438c6717b756bd995d7108bef21a5ac0c9b6c77101"],"receipt":"Rewarded"}],"id":1}%
```

#### `ledger_getTransactions`

This method retrieves transactions. It takes two arguments, a list of `TxIdentifiers`s and an optional `QueryMode`. If
no query mode is provided,
this list of identifiers may be flattened: `"params":[[7]]` and `"params":[7]` are both acceptable,
but `"params":[7, "Compact"]` is not.

**Example Query:**

```shell
$ curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getTransactions","params":[[{ "batch_id": 1, "offset": 0}]],"id":1}' http://127.0.0.1:12345

{"jsonrpc":"2.0","result":[{"hash":"0x191d87a51e4e1dd13b4d89438c6717b756bd995d7108bef21a5ac0c9b6c77101","event_range":{"start":1,"end":1},"receipt":"Successful"}],"id":1}
```

This response indicates that transaction `1` emitted no events but executed successfully.

#### `ledger_getEvents`

This method retrieves the events based on the provided event identifiers.

**Example Query:**

```shell
$ curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getEvents","params":[1],"id":1}' http://127.0.0.1:12345

{"jsonrpc":"2.0","result":[null],"id":1}
```

This response indicates that event `1` has not been emitted yet.

## Testing with specific DA layers

Check [here](./README_CELESTIA.md) if you want to run with dockerized local Celestia instance.

## License

Licensed under the [Apache License, Version 2.0](../../LICENSE).

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this repository by you, as defined in the Apache-2.0 license, shall be
licensed as above, without any additional terms or conditions.
