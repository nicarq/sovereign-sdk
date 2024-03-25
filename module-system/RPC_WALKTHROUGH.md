## Enabling RPC via Module System Macros

In the Module System, we provide handy macros to make it easy to generate RPC server implementations. In this document,
we'll walk you through all of the steps that you need to take to enable RPC if you're implementing your rollup
from scratch.

There are 5 steps that need to be completed to enable RPC on the full node:

1. Annotate your modules with `rpc_gen` and `rpc_method`.
2. Annotate your `native` `Runtime` with the `expose_rpc` macro.
3. Import and call `get_rpc_methods` in your full node implementation.
4. Configure and start your RPC server in your full node implementation.

### Step 1: Generate an RPC Server for your Module

To add an RPC method to a module, simply annotate the desired `impl` block with the `rpc_gen` macro and tag each
method you want to expose with the `rpc_method` annotation. As noted in its `rustdoc`s, the `rpc_gen` macro
has identical syntax to [`jsonrpsee::rpc`](https://docs.rs/jsonrpsee-proc-macros/0.18.2/jsonrpsee_proc_macros/attr.rpc.html)
except that the `method` annotation has been renamed to `rpc_method` to clarify its purpose.

```rust
// This code goes in your module's rpc.rs file
use sov_modules_api::macros::rpc_gen;

#[rpc_gen(client, server, namespace = "bank")]
impl<S: Spec> Bank<S> {
    #[rpc_method(name = "balanceOf")]
    pub(crate) fn balance_of(
        &self,
        user_address: S::Address,
        token_id: TokenId,
        working_set: &mut WorkingSet<S>,
    ) ->  RpcResult<BalanceResponse> {
    ...
    }

    #[rpc_method(name = "supplyOf")]
    pub(crate) fn supply_of(
        &self,
        token_id: TokenId,
        working_set: &mut WorkingSet<S>,
    ) -> RpcResult<TotalSupplyResponse> {
     ...
    }
}
```

This example code will generate an RPC module which can process the `bank_balanceOf` and `bank_supplyOf` queries.

Under the hood `rpc_gen` and `rpc_method` create two traits - one called <module_name>RpcImpl and one called <module_name>RpcServer.
It's important to note that the \_RpcImpl and \_RpcServer traits do not need to be implemented - this is done automatically by the SDK.
However, they do need to be imported to the file where the `expose_rpc` macro is called.

### Step 2: Expose Your RPC Server

The next layer of abstraction where we need to think about RPC is the `Runtime`. Just because a module defines
some RPC methods doesn't necessarily mean that we want to use them. So, when we're building a `Runtime`, we have
to enable RPC servers of the modules.

```rust
// This code goes in your state transition function crate. For example demo-stf/runtime.rs

use sov_bank::{BankRpcImpl, BankRpcServer};

#[cfg_attr(
    feature = "native",
    expose_rpc(DefaultContext)
)]
#[derive(Genesis, DispatchCall, MessageCodec, DefaultRuntime)]
#[serialization(borsh::BorshDeserialize, borsh::BorshSerialize)]
pub struct Runtime<S: Spec> {
    pub bank: sov_bank::Bank<S>,
    ...
}
```

Note that`expose_rpc` takes a tuple as argument, each element of the tuple is a concrete Context.

### Step 3: Instantiate RPC Methods

Now that we've implemented all of the necessary traits, a `get_rpc_methods` function will be auto-generated.
To use it, simply import it from your state transition function. Given access to `Storage`, this function instantiates
[`jsonrpsee::Methods`](https://docs.rs/jsonrpsee/latest/jsonrpsee/struct.Methods.html) which your full node can
execute.

```rust
// This code goes in your full node implementation. For example demo-rollup/main.rs
use demo_stf::runtime::get_rpc_methods;

#[tokio::main]
fn main() {
	// ...
    let mut app = App...;

    let storage = app.get_storage();
    let methods = get_rpc_methods(storage);
	// ...
}
```

### Step 5: Start the Server

The last step is simply binding our generated `jsonrpsee::Methods` to a port:

```rust
async fn start_rpc_server(methods: RpcModule<()>, address: SocketAddr) {
    let server = jsonrpsee::server::ServerBuilder::default()
        .build([address].as_ref())
        .await
        .unwrap();
    let _server_handle = server.start(methods).unwrap();
    futures::future::pending::<()>().await;
}

#[tokio::main]
fn main() {
	// ...
    let mut demo_runner = App...;

    let storage = demo_runner.get_storage();
    let methods = get_rpc_methods(storage);

    let _handle = tokio::spawn(async move {
        start_rpc_server(methods, address).await;
    });

}
```

## Enabling Archival queries for RPC

- We use `working_set: &mut WorkingSet<S>` in order to query state. `WorkingSet` has a function `working_set.set_archival_version(v)` where v is of type `u64` and represents the block height.
- Once the `set_archival_version` is called, the working_set is configured to query against the state at height `v`.
- To modify an RPC query of the form

```rust
pub fn balance_of(
         &self,
         user_address: S::Address,
         token_id: TokenId,
         working_set: &mut WorkingSet<S>,
     ) -> RpcResult<BalanceResponse> {
    Ok(BalanceResponse {
        amount: self.get_balance_of(user_address, token_id, working_set),
    })
}
```

We need to make the following changes

```rust
pub fn balance_of(
         &self,
         version: Option<u64>,
         user_address: S::Address,
         token_id: TokenId,
         working_set: &mut WorkingSet<S>,
     ) -> RpcResult<BalanceResponse> {
    if let Some(v) = version {
        working_set.set_archival_version(v)
    }
    Ok(BalanceResponse {
        amount: self.get_balance_of(user_address, token_id, working_set),
    })
}
```

- NOTE: `set_archival_version` handles configuring `WorkingSet` for both JMT state as well as accessory state

## Querying events from the RPC

Events are emitted at the module level, but queries for events are at the ledger level (as a design decision since events are not currently merkelized / stored in the JMT. They are stored in a flat KV structure)
The following queries are currently supported and can be seen in [server.rs](../full-node/sov-ledger-rpc/src/server.rs):

```text
ledger_getEvents (multiple events by corresponding event numbers)
ledger_getEventByNumber (single event by event number)
ledger_getEventsByKey (paginated events by key)
ledger_getEventsByModuleAddress (paginated events by module address)
ledger_getEventsByTxnHash (paginated events by transaction hash)
```

Below we provide examples of how to query using three of the calls (`ledger_getEventsByTxnHash` and `ledger_getEventsByKey`, `ledger_getEventsByModuleAddress`)

### Fetching the module address

There is one other RPC call to be aware of in relation to events. Every module has an address and it can be used to query events from a specific module. The ability to get the module address is auto-generated as an rpc method for all modules

```text
<namespace>_moduleAddress
```

In order to fetch the module address of the `bank` module we use the namespace that we set in [rpc.rs](../module-system/module-implementations/sov-bank/src/rpc.rs) which is also `bank`. We use the following API call

```bash
$ curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"bank_moduleAddress","params":{},"id":1}' http://127.0.0.1:12345
{"jsonrpc":"2.0","result":"sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h","id":1}
```

### ledger_getEventsByTxnHash

When a transaction is submitted using the `sov-cli`, the standard output contains the hashes of each of the transactions submitted

```text
Your batch was submitted to the sequencer for publication. Response: "Submitted 1 transactions"
0: 66d4a27dd46013f88c156d21d16d364f6a5de66effd74155a5b0815475cbdf17
```

The transaction hash can be used to fetch all events emitted when that transaction was executed

```bash
$ curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getEventsByTxnHash","params":["66d4a27dd46013f88c156d21d16d364f6a5de66effd74155a5b0815475cbdf17"],"id":1}' http://127.0.0.1:12345
{"jsonrpc":"2.0","result":[{"event_value":{"TokenCreated":{"token_id":"token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6"}},"module_name":"bank","module_address":"sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h"}],"id":1}%
```

### ledger_getEventsByKey

The event key for all `CreateToken` calls in the bank module is `token_created` (non-unique).

```bash
$ curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getEventsByKey","params":["token_created",null,null,1,null],"id":1}' http://127.0.0.1:12345
{"jsonrpc":"2.0","result":{"events_response":[{"event_value":{"TokenCreated":{"token_id":"token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6"}},"module_name":"bank","module_address":"sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h"}],"next":null},"id":1}
```

The parameters can be inferred from [rpc.rs](../full-node/sov-db/src/ledger_db/rpc.rs), and [server.rs](../full-node/sov-ledger-rpc/src/server.rs)

```rust
fn get_events_by_key<E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>>(
    &self,
    event_key: &str,
    module_address: Option<&str>,
    txn_range: Option<(u64, u64)>,
    num_events: usize,
    next: Option<&str>,
) -> Result<PaginatedEventResponse, Error> {
// Implementation elided
}
```

The params passed in the curl call correspond to each of the function arguments. We pass the event_key `token_created` and we're only fetching 1 event so num_events is set to 1 while the remaining fields are `null`.

```text
"params":["token_created",null,null,1,null]
```

We can optionally provide the module address as well to get the same result (this helps in case there are conflicting keys across modules). The module address for `bank` can be obtained as explained earlier using the `bank_moduleAddress` rpc call.

```bash
$ curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"bank_moduleAddress","params":{},"id":1}' http://127.0.0.1:12345
{"jsonrpc":"2.0","result":"sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h","id":1}

$ curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getEventsByKey","params":["token_created","sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h",null,1,null],"id":1}' http://127.0.0.1:12345
{"jsonrpc":"2.0","result":{"events_response":[{"event_value":{"TokenCreated":{"token_id":"token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6"}},"module_name":"bank","module_address":"sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h"}],"next":null},"id":1}
```

`getEventsByKey` also optionally allows filtering by a transaction number range

### ledger_getEventsByModuleAddress

`getEventsByModuleAddress` is similar to the previous call, but is more suitable for use cases where all the events from a specific module need to be fetched.
The parameters can be inferred from [rpc.rs](../full-node/sov-db/src/ledger_db/rpc.rs)

```rust
    fn get_events_by_module_address<
        E: BorshDeserialize + Into<sov_rollup_interface::rpc::Event>,
    >(
        &self,
        module_address: &str,
        num_events: usize,
        next: Option<&str>,
    ) -> Result<PaginatedEventResponse, Error> {
// Implementation elided
}
```

The output is identical to the previous calls because we have generated only one event (for the `CreateToken` call)

```bash
 curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"ledger_getEventsByModuleAddress","params":["sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h",1,null],"id":1}' http://127.0.0.1:12345
{"jsonrpc":"2.0","result":{"events_response":[{"event_value":{"TokenCreated":{"token_id":"token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6"}},"module_name":"bank","module_address":"sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h"}],"next":null},"id":1}
```
