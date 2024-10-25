## 2024-10-25
- #1748 adds a `tx_hash` field to the object format used by `/sequencer/events/ws`.

## 2024-10-24
- #1738 Adds a new field `gas_payer` to `StandardProvenRollupCapabilities`. To retain the previous behavior, use `self.bank` in this field. If you wish to opt into the new Paymaster module (which the sequncer to pay gas on behalf of users), add the  `sov_paymaster::Paymaster` module to your runtime and use it as the `gas_payer` instead.

## 2024-10-23
- #1730 Renames the `sov_modules_api::EnumUtils` function to `NestedEnumUtils` to better reflect its purpose.

## 2024-10-16

## 2024-10-18
- #1676: `SequencerRegistry`: the `minimal_bond` is determined by the batch size.
Consumers of the SDK should update their configuration files accordingly (`minimum_bond` was removed from the `sequencer_registry.json`).

## 2024-10-17
-  #1667: `stf-blueprint`: Extract the authentication logic into a separate stage. This change will break SDK consumers who rely on how the sequencer is penalized for processing invalid transactions.
- #1674 Increase users' balances on the rollup. Consumers of the SDK should update their configuration files accordingly.

## 2024-10-16
- #1665 Refactors and cleans-up the `Kernel` new capabilities. Consumers of the SDK may need to implement the `KernelSlotHooks` over the `Runtime` using the default implementation if that is not already the case.
- #1671 Adds `Borsh`, `SchemaGenerator`, `Arbitrary`,  bounds to Da layer address types. It also changes the `SequencerRegistry::CallMessage` type to use `Da::Address` instead of `Vec<u8>` where applicable.

## 2024-10-11
- #1636 Removes the `Kernel` structure from the `StfBlueprint`. It also adds and implements the `HasKernel` trait on the `Runtime`. Please make sure to remove the explicit dependence on the `Kernel` structure in your `StateTransitionFunction` implementations and use the `Runtime` instead. 

## 2024-10-14
- #1642 Accumulate the sequencer's rewards in its staking account. This change will impact SDK consumers who assumed the rewards were accumulated in the sequencer's `personal` account.
## 2024-10-15
- #1661 Adds `max_authentication_gas` to the `Authenticator` trait and corresponding `MAX_AUTHENTICATION_GAS` constant in `constants.toml`.
This is a breaking change and the consumers of the SDK have to add `MAX_AUTHENTICATION_GAS` to `constants.toml`
- #1660 Remove the slash_sequencer capability. This change is only relevant to the sequencers.
- #1663 Makes all existing `JsonSchema` trait requirements apply in the non-native mode. This allows removal of some inconvenient feature gates. 
## 2024-10-10
- #1607 Fixes querying future rollup height via REST API. Now it returns HTTP 404 instead of data at the rollup head.

## 2024-10-05
- #1624 Adds `DaSpec` as an associated type of `Spec` and removes it from every other type inside the module system. See the changes to the demo-rollup here for an example migration: https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/1624/files#diff-d9126f60816d820a29c0bf89e154c54c031f9fec4490301d08c3a3f2b39310e2
- #1610 requires a *direct* dependency on the `strum` crate for any packages that define a `Runtime` struct, or a dev-dependency on `strum` if the package uses `sov-test-utils` to generate a test runtime.
## 2024-10-05
- #1624 Adds `DaSpec` as an associated type of `Spec` and removes it from every other type inside the module system. See the changes to the demo-rollup here for an example migration: https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/1624/files#diff-d9126f60816d820a29c0bf89e154c54c031f9fec4490301d08c3a3f2b39310e2

- #1581 Fixes misalignment of rollup_height and JMT version. Genesis data is available at `rollup_height=0` via REST API.
## 2024-10-05
- #1619 Allows the Kernel information to be immediately available from the transaction context in the non-preferred sequencer mode. Users may experience breaking changes if they were relying on the previous behavior - ie the Kernel information written in the slot _i_ would only be available in the slot _i+1_.
- #1581 Fixes misalignment of rollup_height and JMT version. Genesis data is available at `rollup_height=0` via REST API.

## 2024-10-11

- #1630 allows customization of rollup address prefixes (also known as "HRP") by setting the `ADDRESS_PREFIX` constant in `constants.toml`. E.g.:
  
  ```toml
  [constants]
  ADDRESS_PREFIX = { const = "myrollup" }
  ```

## 2024-10-08
- #1599 removes the kernel macros and moves the Kernel modules inside the runtime. This may be a breaking change if users have implemented their own Kernels (we have removed the `Genesis` configuration from the `Kernel` trait) or if they have used the `Kernel` macro inside their modules (`KernelModule` derive macro, `kernel_module` attribute).

- The two type generics of `ApiState` have been inverted, e.g. `ApiState<Bank<S>, S>` is now `ApiState<S, Bank<S>>`, and the second generic defaults to `()`.
## 2024-10-08
- #1589 Revamp universal wallet generation code
This fully integrates the new `UniversalWallet` macro, which was previously present as beta versions. This macro should be derived on any types which may be part of the `RuntimeCall` or transaction types of a rollup; once derived, a schema for those types can be generated and used in rollup-agnostic wallets. The derivation should be gated by `#[cfg(feature = "native)]`.

For module authors, the `UniversalWallet` macro should be derived on the `CallMessage` type of your module(s), and subsequently any types used in the `CallMessage` (the compiler will enforce this). Any modules that already derived the previous implementation of the macro should double-check the import path: it is available at `sov_modules_api::macros::UniversalWallet`; the derivation and import must also be `"native"` feature-gated as above.

The underlying traits that the macro implements are also available to be used as members of the `sov_modules_api::sov_universal_wallet::*` module. The `SchemaGenerator` trait can be implemented manually as an alternative to deriving the macro, and the `OverrideSchema` trait can be used to manually designated a different type's implementation to be used for a given type - provided their `borsh` encodings are identical.

The macro is additionally available in `sov_rollup_interface` at the path `sov_rollup_interface::sov_universal_wallet::UniversalWallet`.

## 2024-10-07
- #1588 changes the signature of the `GasEnforcer` capability, adding one new method `try_reserve_gas_for_proof` and altering the signature of `try_reserve_gas` to include the entire `Context` instead of just the sender address. If you're using the standard capabilities, no change is needed. If you implemented your own `GasEnforcer` capability, simply copy-paste your old implementation as `try_reserve_gas_for_proof`, and use `context.sender()` in place of the `sender` argument within `try_reserve_gas`.
- #1584 extract transaction authentication to a separate methods.

## 2024-10-05
- #1581 Fixes misalignment of rollup_height and JMT version. Genesis data is available at `rollup_height=0` via REST API.

## 2024-10-04
- #1577 introduces a new `GasMeter::gas_info` method to the `GasMeter`. This is a breaking change for SDK consumers. Any code that previously used `gas_meter.gas_price()` will need to be updated to `gas_meter.gas_info().gas_price`, and similarly for `remaining_funds` and `gas_used`.

## 2024-10-03
- #1525 Adds `title` to `IntOrHash` enum variants in OpenAPI spec for LedgerAPI. This improves generated clients in some cases.
- #1568 `TransactionAuthorizer` capability no longer charges for the gas.
This is a breaking change for the consumers of the SDK. The capabilities accept 
`TxScratchpad` instead of `PreExecWorkingSet`.
- #1565 `GasEnforcer::try_reserve_gas` now accepts `TxScratchpad` instead of `PreExecWorkingSet`. This is a breaking change for the consumers of the SDK. 

## 2024-10-02
- #1557 Simplifies the internal representation of the `ChainState` module and ensures that `gas_price` updates can be immediately accessible through the `Kernel` interface. The gas price accessible from the kernels should now update immediately after a slot is processed. This can potentially break tests that rely on the previous behavior (ie, 1-slot delay for the gas update).
- #1547 Adds support for `DerivedHolder` in the `TokenHolder`. 
With this change token holder can be generated programmatically.

- #1549 introduces **an optional** `public_address` field to `runner_config`. 
   If this `public_address` is provided, it will be used as the server entry in the OpenAPI specification and UI. 
   This allows for the correct rendering of the server address in the OpenAPI spec when the rollup node is running behind a proxy.
   
With this change token holder can be generated programmatically. Example DerivedHolder: "derived_1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqgfk7l6"

- #1548 Moves OpenAPI response objects to schemas so it has name and client generators have better output.
  Adds missing 404 case for StateVecElementResponse

## 2024-10-01
- #1539 Removes JSON-RPC APIs from all modules, except `sov-evm` (note that JSON-RPC support for custom modules was **not** removed). **Users should switch to their REST API counterparts**. Additionally, code generated by `#[derive(HasRestApi)` no longer depends on `sov-state`, so the dependency can usually be removed.
- #1546 Removes `salt` from `CallMessage::CreateToken` in the bank module. This is are the breaking changes for the clients of the API:
1. The salt hast to be removed form all the call messages. 
2. The "GAS_TOKEN_ID" was updated from `token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6` to `token_1nyl0e0yweragfsatygt24zmd8jrr2vqtvdfptzjhxkguz2xxx3vs0y07u7` 

- #1544 `SequencerAuthorization::penalize_sequencer` capability now takes `TxScratchpad` as an argument.
 
## 2024-09-30
- #1537 Removes need to define swagger-ui in `Runtime::endpoints` implementation. Now it works out of the box by `sov_modules_rollup_blueprint::register_endpoints`.
- #1542 removes support for `config_bech32!`, and merges it into the main proc-macro `config_value!`.

  ```toml
  [constants]
  # ...

  # BEFORE (Rust: `config_bech32!("MY_CONST", TokenId)`)
  MY_CONST = "token_1qwqr2h2e5g961t4f2m1qt3t3d7xx7r4jchjc9ey5pe1r5u8ers9ts"

  # AFTER (Rust: `config_value!("MY_CONST")`)
  MY_CONST = { bech32 = "token_1qwqr2h2e5g961t4f2m1qt3t3d7xx7r4jchjc9ey5pe1r5u8ers9ts", type = "TokenId" }
  ```

  Moreover, the `config_value!` macro (1) is now not `const` expression -compatible by default and (2) supports overriding values in tests via env. variables. See the docs of `sov_modules_api::macros::config_value!` for a guide.

## 2024-09-26
- Changes the argument to the `Runtime::endpoints`, `HasRestApi::rest_api` and `get_rpc_methods` functions to `sov_modules_api::rest::ApiState<(), S>`
- Renames the `ApiStateAccessor::new` function to `from_storage` and adds the `Kernel` as a second argument.

- #1518 Removes the hardcoded `Vec<u64>` type to represents the gas price inside the `BatchReceipt` struct. The `StateTransitionFunction` trait now also has an associated `GasPrice` type to represent the notion of gas price inside `sov-rollup-interface`. Consumers of the SDK should ensure they replace their use of the `Vec<u64>` with a new generic `GasPrice` type and to specify the associated `GasPrice` type inside their implementations of the `StateTransitionFunction`. 

## 2024-09-25
- Adds new `JsonSchema` bounds on all `Address` types and `DaService::Config`. 
- Makes a significant refactor to the `BatchBuilder` trait
- Changes the signature of the `create_endpoints` function of `RollupBlueprint` to be `async
- Removes the third generic parameter from `RollupConfig` and changes the type of the second one from `DaService::Config` to `DaService`
- Renames the `sequencer_address` item in the `rollup_config.toml` to `da_address` to clarify the usage. 
- Changes the syntax of the `sequencer` section in `rollup_config.toml`. Now, callers must include one of `[sequencer.standard]` or `[sequencer.preferred]` there.


- #1510 Removes the `from_slice` method from the `GasArray` trait in favor of implementations of `From<[u64; config_value!("GAS_DIMENSIONS")]` and an implementation of `TryFrom<Vec<u64>>`. Consumers of the SDK should ensure they replace their use of the `from_slice` method with `From<[u64; config_value!("GAS_DIMENSIONS")]` (preferred solution, for type-safety reasons), otherwise they can use the `TryFrom<Vec<u64>>` method.
- #1514 Refactors `TestRunnerWithKernel::config.override_next_header_timestamp` to instead be `TestRunnerWithKernel::config.freeze_time`. It will now set the timestamp of all blocks to the provided timestamp instead of only the next block.
- #1509 Adds a `GasSpec` trait that is blanket implemented over the `Spec` trait. This adds a `GAS_DIMENSIONS` constant to be specified in the `constants.toml` file and removes all the other gas dimensions. Consumers of the SDK should ensure they only use one gas dimension and they specify the value of the `GAS_DIMENSIONS` constant in the `constants.toml` file.
- #1493 Makes it mandatory to manually set the `max_fee` in the CLI wallet. Transactions that don't set that value won't be accepted by the CLI. Consumers should ensure there is always a value set for the `max_fee` when using the CLI.

## 2024-09-20

Adds a new `arbitrary::Arbitrary` bound on the `Spec::Address` associated type and the `PrivateKeyExt` trait when `arbitrary` feature is enabled.

- #1470 Expresses all the bonds from the `AttesterIncentives`, the `ProverIncentives` and the `SequencerRegistry` in terms of multi-dimensional gas units instead of raw token value. That way the bond value of the rollup `sequencers/attesters/challengers/provers` will become dependent of the fluctuations of the gas price (which, in turn, makes the security of the rollup *independent* of the gas price fluctuations).
- #1480 Add the AttestationsManager, which is responsible for managing Attestation creation. This PR will break the `sov-rollup-starter` in several ways:
  1. Updates `prover_address` in rollup_config.toml.
  2. Adds a new method and an associated type to `FullNodeBlueprint`
- #1467 Adds the prefix `sov_` to all metrics emitted by the rollup.
- #1472 Export `ModuleRestApi` at the top level of `sov_modules_api` rather than `sov_modules_api::macros::ModuleRestApi`
- #1474 Add a test for the optimistic workflow
- #1463 Brings several fixes and changes in generated REST API for Runtime with modules:
   - Breaking: If StateValue or elements in StateVec and StateMap are not found HTTP 404 is returned instead of HTTP 200 with null value.
   - Correct return type for `StateValue` in OpenAPI spec.
   - Correct bech32 regex for `TokenId` and `ModuleId` in OpenAPI spec.
   - List of modules endpoint is brought back after accidental disabling.
   - Added `operationId` to custom REST API endpoints in Bank module.
- #1464 Improve the code structure of the integration tests
- #1461 Add `ProcessManager` that manages processes consuming StateTransitionInfo.
- #1459 Modifies the `TxState` trait to make it compatible with the `ApiStateAccessor` so that it becomes possible to test module call methods directly using the `ApiStateAccessor`.
-  #1457 Adds the `sov-stf-runner/processes` module and moves the `ProofManager`, `ProverService`, and `sov-stf-info-manager` there.
- #1456 Fixes infinite redirect in /swagger-ui endpoint.
- #1454 LedgerDb: use async_get to get data from the db.
- #1446 Integrate stf_info_manager in runner.rs
- #1450 Renames two related traits: `RuntimeAuthorization` becomes `TransactionAuthorizer`; `RuntimeAuthentiation` beomces `TransactionAuthenticator`. The `runtime_authorization` method on the `HasCapabilities` trait is also renamed to `transaction_authorizer`. 
- #1443 Move StateTransitionInfo channel creation to `FullNodeBlueprint`.
- #1442 Makes stf-runner handle trailing slashes without throwing HTTP 404. 
- #1438 enforces a consistent `snake_case` convention for JSON. **This requires updating the value `operating_mode` in `chain_state.json`** from `"Zk"` to `"zk"`. 
- #1436 Adds `get_operating_mode` to the `FullNodeBlueprint`. This tells the rollup whether it is running in zk or optimistic mode.
- #1432 stf_info_manager: Simplify the Db pruning logic
- #1428 Make stf_info_manager compatible with the StorageManager workflow.
- #1423 Adds Db pruning in the stf_info_manager.
- #1424 Breaking change in `DaService`. An associated type "`TransactionId`" is added to `DaSpec` and the return type of of `send_transaction` and `send_aggregated_zk_proof` has changed as follows:
  ```rust
  // New type:
  pub struct SubmitBlobReceipt<T: Debug + Clone> {
    // Canonically calculated blob hash
    pub blob_hash: HexHash,
    // DA native transaction id.
    pub transaction_id: T,
  }
  
  pub trait DaService {
     // rest is omitted...
     
     async fn send_transaction(
        &self,
        blob: &[u8],
        fee: Self::Fee,
     ) -> Result<SubmitBlobReceipt<<Self::Spec as DaSpec>::TransactionId>, Self::Error>;
  
     async fn send_aggregated_zk_proof(
        &self,
        aggregated_proof_data: &[u8],
        fee: Self::Fee,
    ) -> Result<SubmitBlobReceipt<<Self::Spec as DaSpec>::TransactionId>, Self::Error>;=  
  }
  ```
- #1416 Adds the correct version of the kernel state root to the `VisibleRoot`. We have to be careful to add the correct `kernel` state root to the `VisibleRoot` because the `historical_state_transitions` map that contains the roots is indexed by `true_rollup_height`. Hence, we have to make sure we get the `kernel_state_root` corresponding to the current `virtual_height` accessible from the user space.
- #1418 Add back pressure mechanism to the StfInfoManager 
- #1409 Removes the `override_sequencer` field from test case structures and `TestRunner::execute` & `TestRunner::simulate` in favor of a single central location.
    - Instead of passing this param you should set `runner.config.sequencer_da_address` right before executing your test case. Also note that the semantics have changed, previously
      following test cases would revert back to the old sequencer da address - this is no longer the case. If you need this behavor you should save and restore the sequencer address.
      Example: `https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/1409/files#diff-2ce1f1e8ed6a6e93b23ddeee55eb317ff55dcf8a6fa1fb544916f3f67ee7b9abR257`
- #1415 Add StfInfoManager, which manages data related to state transitions. 
- #1413 Remove borsh bounds from StateTransitionWitness.
- #1407 Renames `transition_num` to `rollup_height` in the `AttesterIncentives` module.
- #1405 Fixes rendering of OpenAPI spec to match derived REST API endpoints for modules. 
- #1406 Removes the `Authenticator` trait and shifts responsibility for encoding authentication information into the `TransactionAuthenticator`. All
references to the `Authenticator` trait or `ModAuth` struct are replaced with references to `TransactionAuthenticator` or the concrete runtime type of the rollup as appropriate.
- #1398 Makes `StateCheckpoint`, `TxScratchpad` and `KernelStateAccessor` generic over `Storage` instead of `Spec`. These structs having no dependency on the `Spec`, this restricts the scope of the generics.
- #1397 Removes the DA generic from the `Kernel` trait.
- #1393 makes the new version of the kernel state accessors type-safe by preventing them to be built using the `From` trait of the `KernelStateAccessor`, but rather using a new `accessor` method in the `Kernel` trait. It also moves the `true_slot_height` out of the `StateCheckpoint` to the `KernelStateAccessor`
- #1378 Plugs in the new state accessors used in soft-confirmation. From now on, accessors such as the `StateCheckpoint` can access `VersionedStateValues` in the storage using the same mechanism as soft-confirmations.
- #1381 Adds an associated `Input` type the `TransactionAuthenticator` trait and expects that type as the argument to `authenticate`. It also adds a new method to the trait `fn encode_default_tx()` which implemetns the runtime-specific notion of a "standard" authentication path. 
- 1392 Simplify the test in prover/attester incentives.
- #1399 `AttesterIncentive`: Add balance checks to the challenger.rs tests.
- #1395 Add balance check to the `attestation_processing` test.
- #1392 Simplify the test in prover/attester incentives.
- #1388 Make the naming in the sov-test-utils more consistent.
- #1383 Extend `prover-incentoves::test_valid_proof` test to check gas usage. 
- #1370 Makes some light changes to the `GenesisStateAccessor` to become a wrapper around `StateCheckpoint`. 
- #1362 Adds a versioned state accessor to be used in the soft confirmation context. This versioned state accessor is append-only and should be initialized at genesis to be properly used.
- #1366 Replaces the `Delta` by a `StateCheckpoint` inside the `TxScratchpad`. This is because we are going to add fields to the `StateCheckpoint` (`virtual_height`, `true_height`) - this will allow to propagate the values up to the `WorkingSet`.
- #1370 Makes some light changes to the `GenesisStateAccessor` to become a wrapper around `StateCheckpoint`. 
- #1356 Adds OpenAPI spec for sov-bank custom REST API endpoints.
- #1374 Add gas handling in the stf-blueprint::process_proof.
- #1377 Enables REST API endpoints for `AttesterIncentives` module.
- #1369 add an extra generic type to the `sov_evm::authenticate` function. This generic enables conversion between `reth::address` and `Spec::Address`. This change is only breaking for users of the EVM module.
- #1365 Simplify sequencer reward workflow.
- #1378 Plugs in the new state accessors used in soft-confirmation. From now on, accessors such as the `StateCheckpoint` can access `VersionedStateValues` in the storage using the same mechanism as soft-confirmations.
- #1362 Adds a versioned state accessor to be used in the soft confirmation context. This versioned state accessor is append-only and should be initialized at genesis to be properly used.
- #1366 Replaces the `Delta` by a `StateCheckpoint` inside the `TxScratchpad`. This is because we are going to add fields to the `StateCheckpoint` (`virtual_height`, `true_height`) - this will allow to propagate the values up to the `WorkingSet`.
- #1360 attester-incentive & prover-incentives: Ensure that the helper methods only read the state.
- #1358 Adds a mechanism for individually overriding capabilities on the `HasCapabilities` trait.
    - All usages of `runtime.capabilities()` should updated to the capability name in `snake_case`.
      For example, `runtime.capabilities().try_reserve_gas` should become `runtime.gas_enforcer().try_reserve_gas`.
- #1344 Adds revertable errors to the `sov-attester-incentives` module.
- #1353 Removes the `Batch` argument from the `begin_batch_hook` to allow the preferred sequencer to process batches.
- #1352 adds a way to retrieve the `base_fee_per_gas` in the sequencer at the current virtual height. To do that we added a method to the `ChainState` that returns the current `base_fee_per_gas` at the virtual slot and a method to the `KernelSlotHooks` trait that allows easy access from the `Kernel`. It also removes the output from the `begin_slot` hook in the `KernelSlotHooks` trait, to allow more consistency accross the hooks (they shouldn't return anything).
- #1335 Removes the previously deprecated method `StateCheckpoint::to_working_set_deprecated`. If you still use this method, please migrate your code to the testing framework available in `sov_test_utils`.
- #1332 Remove events from AttesterIncentives capabilities 
- #1328 Add reverts support in `prover-incentives` module.
- #1329 Adds custom REST API endpoints to `sov-nft` module. Migrates `test-harness` to REST API, so `rpc_url` is not accepted anymore, and `rest_url` renamed to `node_url`. `genesis_dir` parameter is also removed, as data is sourced from REST API now. 
- #1322 Remove ProcessAttestation & ProcessChallenge call messages from the sov-attester-incentives.
- #1323 Modifies the `Event`, `DispatchCall`, `MessageCodec`, and `CliWallet` proc-macros to generate `enum`s with `PascalCase` instead of `snake_case`, as per Rust naming conventions.
- #1320 Updates challenger tests in attester-incentives module for invalid challenges.
- #1314 Updates challenger tests in attester-incentives module.
- #1314 Updates challenger tests in attester-incentives module.
- #1316 Renames `sov-nft-module` to `sov-nft`. Runtimes that use it need update `Cargo.toml` and runtime definition.
- #1312 Made changes to the module structure of the `sov-rollup-interface` crate. You'll now find full-node-related code and interfaces in `sov_rollup_interface::node`; we suggest using `cargo doc` to better navigate the new crate structure.
- #1306 Updates tests in attester-incentives part 2.
- #1276 Migrates sov-cli to using raw REST API requests. `rpc` subcommand replaced with `api`.
- #1308 Removes the associated type `FullNodeBlueprint::DaConfig` and moves it over to `DaService::Config`. This is most often used as the second generic of `RollupConfig`, which should become `RollupConfig<..., <Self::DaService as DaService>::Config, ...>`.
- #1299 Updates tests in attester-incentives
- #1275 Adds an `ExecutionContext` enum to `stf::apply_slot` and `Context::new`. This enum allows callees to distinguish between sequencer execution and normal "full node" execution.
- #1297 Renames the `?height=...` query parameter in the REST API to `?rollup_height=...`.
- #1290 Refactors slashing mechanism in the prover incentives module.
- #1287 Breaking Change: Adds `FromStr` trait bound to a `BlockHashTrait`. 
- #1291 New flag `--skip-if-present`: `sov-cli keys import --skip-if-present` allows to skip importing file if was previously imported.
- #1264 Makes testing-harness generic over runtime.
- #1267 Fixes HTTP 500 in Ledger REST API `slots/<number>` endpoint.
- #1261 Return Attestation and Challenge in ProofReceipt 
- #1252 Add `Attestation` data in the `ProofReceiptContents::Attestation` variant.
- #1245 Add `sov-attester-incentive` to the rollup's `Runtime`. Adds relevant config files.
- #1238 Adds a custom REST API endpoint to bank module, allowing query balance. Fixes `ModuleRestApi` derive macro to correctly pickup specialized implementation of `HasCustomRestApi` trait.
- #1234 Add support for `process_attestation & process_challenge` capabilities in the `STF blueprint`.
- #1233 Add support for the optimistic workflow in the `STF blueprint`.
- #1228 Converts sov-attester-incentive to capability.
- #1223 Adds logging of response times for celestia adapter. Minimal configuration: `RUST_LOG="info,sov_celestia_adapter::da_service=trace,jsonrpsee=trace,jsonrpsee-http=info,jsonrpsee-client=info"`
- #1227 Changes public API in `StateDb`. `StateDb::materialize_preimages` and `StateDb::materialize_node_batches` are not generic anymore and now take 2 parameters for `kernel` and `user` namespace data.
- #1199 renames `sov_state::EncodeKeyLike` to `sov_state::EncodeLike`, so that it can also be used for values when interfacing with `StateValue`, `StateMap`, and `StateVec`.
- #1169 removes the `#[serialization]` attribute consumed by the `DispatchCall` and `event` macros. Now, the enums generated by those macros derive `borsh` and `serde` by default. Use `#[event(no_default_attrs)]` and `#[dispatch_call(no_default_attrs)]` to opt out of the defaults. Use `#[event({attr})]` or `#[dispatch_call({attr})]`to add an attribute to the generated enums.
- #1172 adds a new configuration section `[sequencer.batch_builder]` to the rollup configuration TOML file. Inside this section, you must specify the sequencer DA address. This was previously set in the `[da]` section as `celestia_own_address`.
- #1073 Changes error handling in `demo-rollup`, so it not panics on error, but high level main function prints error and exits with appropriate code
- #1141 Renames `risc0-cycle-macros` to `sov-cycle-macros` and changes the usage pattern. Callers should typically import the `sov-cycle-utils` crate and use its `macros` module rather than importing `sov-cycle-macros` directly.
- #1109 removes the sequencer tx status notifications `published` and `finalized`, and replaces them with `processed` instead. A `processed` notification instructs the client that the ledger has processed the transaction and it's ready to serve finality data, its receipt, and its events.
- #1146 Adds `DEFAULT_SOV_ROLLUP_LOGGING` constant in `sov-modules-rollup-blueprint` that should be used for default tracing-subscriber setup.
- #1143 Adds new reth crate and removes copy pasted code. `jsonrpsee` and `tokio` have been updated.
- #1149 Adds `FinalizedBlocksBulkFetcher` to stf runner for faster sync.
- #1121 changes the `BatchReceipt`'s `inner` default field and adds the `sequencer_da_address` to it. In particular:
    - Add a new `BatchSequencerReceipt` struct which contains the sequencer's `da_address` and `BatchSequencerOutcome`.
    - Change the `BatchResult` type from `BatchSequencerOutcome` to `BatchSequencerReceipt`.
    - Remove the sender's `Da::Address` field from the `end_batch_hook` because it is now accessible from `BatchResult`
    - Minor stylistic improvements in the `StfBlueprint`.
- #1120 Improve error handling in SequencerRegistry.
- #1135 Removes need for a `secp256k1` patch for risc0, as `k256` crate is used in guest context. Also switches to upstream reth from Sovereign fork
- #1129 changes `StateVec::iter` to return `Result`s to account for state reading errors. The new method `StateVec::collect_infallible` can be used in tests and for infallible state accessors.
- #1108 improve error handling in `ProverIncentives` module.
- #1104 Add `deposit` method to prover incentives module.
- #1106 Upgrades [`reth`](https://github.com/paradigmxyz/reth/) dependencies to `1.0.3`. Still based on Sovereign fork.
- #1003 Removes the `BlobData` enum, requiring sequencers to post the `Batch` struct directly on chain.
- #1076 turns the `FullNoteBlueprint::create_endpoints` and `register_endpoints` functions async. It also modifies the sequencer to emit a new tx status event when transactions are dropped from the mempool.
- #1062 Adds `remove` method to `StateVec`
- #978 Upgrades rust toolchain to 1.79.
- #1047 Use ProverIncentive module in the StfBlueprint for proof verification.
- #1025 Pass the `genesis_state_root` to the `ProverService`
- #1024 removes the associate type `DaService::TransactionId` and replaces it with `BlobReaderTrait::BlobHash`. The type signatures of `DaService::send_aggregated_zk_proof` and `DaService::send_transaction` now return `BlobHash` instead of the unit type.
- #1024 removes the associated type `DaService::TransactionId` and replaces it with `BlobReaderTrait::BlobHash`. The type signatures of `DaService::send_aggregated_zk_proof` and `DaService::send_transaction` now return `BlobHash` instead of the unit type.
- #1021 Moves the `BatchSequencerOutcome` type to the `sov-modules-api` crate.
- #1011 Rename `hooks.rs` to `capabilities.rs` in the `Accounts` module and add `capabilities.rs` for `ProverIncentives` module.
- #1004 Slash the sequencer if the aggregated proof can't be deserialized in the proof processing workflow.
- #1007 adds `slotNumber` to batch objects and `batchNumber` to transaction objects returned by the ledger API.
- #1001 StfBlueprint cleanup: Remove ApplyBatch type.
- #995  Move `SequencerRegistry::hooks` logic to `SequencerRemuneration` capability.
- #1062 Adds `remove` method to `StateVec`
- #966 Adds gas & fees relevant logic to the `STF::process_proof` method.
- #954 Replaces `ProverStorageManager` with `NativeStorageManager`. `StateDb`, `AccessoryDb` and `LedgerDb` now have different constructors.
- #956 Split stf_blueprint into smaller chunks.
- #950 Add metadata about gas & fees to the serialized proof.
- #943 Add `ProofSerializer` trait which allow adding additional metadata to the proof blob.
- #908 refactors the testing framework to achieve the following objectives:
   - Allow the execution of transactions from different modules in one test
   - Allow the execution of multiple batches within one slot
- #927 Refactor common fields in `Transaction`, `UnsignedTransaction` & `Message` into a new `TxDetails` struct.
- #913 Read prover address from file configuration file.
- #910 replaces `constants.json` with `constants.toml` and `constants.test.json` with `constants.testing.toml`. The expected file "schema" and contents are the same, just the file format has changed and you'll just need to translate your file contents from JSON to TOML.
- #906 Simplifies the `UnmeteredStateWrapper` wrapper struct and adds it in the testing framework as a wrapper of the `TxState` trait to prevent test maintainers from charging gas in the hooks.
-  #889 Add `rewarded_addresses` filed to `openapi::AggregatedProofPublicData` 
-  #869 Add prover address to `AggregatedProofPublicData` 
- #904 removes the need to specify event keys when calling `self.emit_event`, because the event key is now generated automatically based on the module name and the event variant (e.g. `Bank/TokenCreated`). You can use `self.emit_event_with_custom_key` to emit events with custom keys.
- #900 Track & charge pre-execution gas costs during direct sequencer registration.
- #881 adds blessed constants in the `constants.toml` to be used to charge gas in the tests. It also removes an unused method `tx_fixed_cost` that got deprecated with the recent changes in gas. The default value of the `MAX_FEE` in the CLI wallet is now `10_000_000`.
- #864 Add prover address to `AggregatedProofPublicData` 
- #885 renames the `rpc.rs` file in the standard SDK module template to `query.rs`.
- #869 Add prover address to `AggregatedProofPublicData` 
- #862 Add `prover_address` to `StateTransitionPublicData` this will allow rewarding the prover.
- #859 Adds a `MeteredSignature` struct wrapper and a `MeteredBorshDeserialize` trait that respectively charges gas for signature verification and borsh deserialization. We have renamed the `hash.rs` file to `metered_utils.rs` inside the `sov-modules-api/common` crate and grouped all the custom metered utils there. 
- #863 Upgrades borsh to version 1.0. If you import borsh in one of your crates, be sure to upgrade as well.
- #856 removes support for the `ledger_*` RPC methods and replaces them with a REST API accessible by default at `http://localhost:12346/ledger`.
- #862 Adds `StateTransitionWitnessWithAddress` struct that is used to pass the prover address to the Prover.
- #853 Make the `ProverIncentives` call methods public and accept prover address explicitly.
- #823 Adds StorableMockDaService, where blocks will be persisted between restarts. `[da]` section for adapter has been changed. 
- #821 removes support for the RPC methods of rollup sequencers, and replaces them with a REST API accessible by default at `http://localhost:12346/sequencer`.
- #851 Add BlobData constructors. 
- #842 `ProofManager` saves zk-proofs received from the `STF` in the db. 
- #839 Refine "ProofProcessor" capability.
- 835 adds a new `MeteredHasher` struct that charges gas for every hash computation. This structure is meant to be used in the module system to charge gas when hashing data.
- #835 adds a new `MeteredHasher` struct that charges gas for every hash computation. This structure is meant to be used in the module system to charge gas when hashing data.
- #828 add `ProofProcessor` capability.
- #813 is a follow-up of #783, it sets the constants that define the costs for gas access to non-zero values. That way, we charge some gas when trying to access the storage and the metered accessors (like the `WorkingSet`) can now run out of gas because of state accesses.
Meaningful changes:
  - We had to change the blessed values for the transaction fee, the initial account balances and the attester/prover stakes. The new values are: `MAX_FEE = 1_000_000`, `INITIAL_BALANCE = 1_000_000_000` and `DEFAULT_STAKE = 100_000`.
  - We had to replace the gas computation of some tests that made strong assumptions about the gas consumed by transaction execution.
- #814 mandates `0x` prefixes for hashes in RPC and REST APIs, whereas before it was optional.
- #809 extend blob-storage to support proof DA namespace.
- #806 simplify new_test_blob_from_batch
- #783 enables gas metering for storage accesses inside the module system. In particular, it makes all the state accessors fallible, and the module code now have to handle the case where the state accessor runs out of gas.
Meaningful changes
  - Making all the state accessors fallible: for instance `state_value.get(&mut state_reader)` now returns `Result<Option<StateValue>, StateReader::Error>`. Depending on the type of the state reader, the error type may be `Infallible` (for unmetered state accessors) or `StateAccessorError` (for metered state accessors)
  - The metered state accessors are: `WorkingSet` and `PreExecWorkingSet` when accessing the provable state; the unmetered state accessors are `TxScratchpad`, `StateCheckpoint`, `KernelStateAccessor`, ... 
  - A summary of all the access control patterns can be found in `sov-modules-api/src/state/accessors/access_controls.rs`
  - A new cargo dependency has been added: `unwrap_infallible`. This allows us to safely unwrap the type `Error<T, Infallible>` which has become ubiquitous because the `Infallible` error type is raised whenever an unmetered state accessor tries to access the state (416 occurrences in our codebase after this PR is merged).
  - The `WorkingSet` use, specially its instantiation with the `WorkingSet::new` pattern has been *significantly* reduced in favor of using the `StateCheckpoint` in the tests. Since the `StateCheckpoint` is an unmetered state accessor, this makes the tests more manageable and maintainable - we can use `unwrap_infallible`. Besides, one now needs to use a special `GenesisStateAccessor` to instantiate the modules at genesis, which needs to be built from the `StateCheckpoint`.
  - temporary `evm` feature that allows converting the `WorkingSet` to an `UnmeteredStateWrapper`. **This is a temporary solution and that feature will be removed once we find a way to connect the EVM and module gas metering**. This is the final task of https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/734.

- #800 `LedgerDb` explicitly returns changes instead of saving them in local cache of underlying `CacheDb`.
- #796 Reuse Batch struct in BatchWithId & PreferredBatch.
- #789 Changes the Transaction receipt type to include the reason a transaction was skipped or reverted. Consumers of the API should now use the `sov_stf_blueprint::TxEffect` as the `TxReceipt` return type for ledger RPC queries.
- #790 Simplify `BlobSelector` code.
- #753 updates default `max_fee` in `sov-cli` to `10_000` (from previous value of `0`)
- #791 adds `Self::TransactionId` to `DaService::send_aggregated_zk_proof`.
- #787 split capabilities into separate modules.
- #770 enforces that transactions are set with the correct chain ID. To get the ID, use `config_value!("CHAIN_ID")`.

- #775 adds a couple of custom error types that will be useful to allow using the `try` pattern within the module system to automatically convert errors into `anyhow::Error`. Meaningful changes:
  - Adding a custom `GasMeteringError` enum that describes the possible causes for error inside the `GasMeter` trait methods
  - Adding a custom `StateAccessorError` enum that describes the situations where accessing the state might fail.
  - Adding the `std::error::Error + Send + Sync` bounds to the associated `Error` type in `StateReader` and `StateWriter` (this allows automatic conversion to `anyhow::Error`
- #766 modifies the RPC interface to accept an `ApiStateAccessor` instead of a `WorkingSet` to prepare the full integration of the gas metering for state accesses. In particular this commit changes the `RPC` macro to accept an `ApiStateAccessor` instead of a `WorkingSet` as an argument to the rpc methods.
- #726 adds Swagger UI -an OpenApi playground- as an endpoint defined inside `sov_modules_stf_blueprint::Runtime::endpoint`.
- #743 adds metered state accessor traits to the `sov-modules-api/state` module.
- #764 changes notifications logic, so ledger notifications arrive after state has been completely updated.
- #750 moves `TransactionAuthenticator, TransactionAuthorizer, and Authenticator` to a separate file in the capabilities module.
- #751 adds `DaServiceWithRetries` wrapper.
   - To use it, make sure your `DaService::Error` type is `MaybeRetryable<anyhow::Error>`. You can then wrap your `DaService` in `DaServiceWithRetries` whilst providing a policy for exponential retries via: `DaServiceWithRetries::with_exponential_backoff(<da-service>, <exponential-backoff-policy>)`. This wrapped `DaService` can then be used normally as a `DaService` anywhere.
   - Use the `MaybeRetryable` return type in your `DaService` to indicate whether a fallibe function maybe retried or not, by returning either `MaybeRetryable::Transient(e)` if you want your functions to be retried in the case of errors, or `MaybeRetryable::Permanent(e)` if you don't want them to be retried.
- #749 makes the rollup generic over the `AuthorizationData` type.
- #742 moves the scratchpad and the state into a submodule of the sov-modules-api and breaks these files into smaller components to make the complexity more manageable.
- #730 Splits `RollupBlueprint` into two traits: `FullNodeBlueprint` and `RollupBlueprint` and feature gates the full-node blueprint behind the `"native"` feature flag. It also reduces the number of required types for the `RollupBlueprint` by making it generic over execution mode. See the diff of `celestia_rollup.rs` for a complete example of a migration.
- #735 updates `minter_address` to `mint_to_address` in `CallMessage::CreateToken` & `CallMessage::Mint`
- #725 removes the `macros` feature from `sov-modules-api`, which is now always enabled even with `--no-default-features`.
- #732 adds the `sov-nonces` module.
- #714 integrates a batch of changes to the `StfBlueprint` and the capabilities. Meaningful changes:
  - Remove arguments of type `SequencerStakeMeter` from the capabilities and replace all of the `(SequencerStakeMeter, StateCheckpoint)` couple of variables by a single `PreExecWorkingSet` which is a type safe data structure that should charge for gas before transaction execution starts.
  - Removing the `ExecutionMode` type from the `StfBlueprint`. It is now replaced by `TxScratchpad`, which is an intermediary type between `WorkingSet` and `StateCheckpoint`. This is useful for the `sov-sequencer` crate because one may want to revert all the changes that happened during transaction execution when simulating a transaction before adding it to a sequencer batch.
  - The entire transaction processing lifecycle is now embedded in the `process_tx` method that is called by both the `apply_batch` in `StfBlueprint` and `try_add_tx_to_batch` in the `sov-sequencer` crate. Hence we now have the following state machine for transaction execution:
        - In `to_tx_scratchpad` we convert the `StateCheckpoint` into a revertable `TxScratchpad` for transaction execution.
        - In `authorize_sequencer` we consume the `TxScratchpad` and build a `PreExecWorkingSet` on success, we stop processing the batch on failure
        - In `try_reserve_gas` we consume the `PreExecWorkingSet` and we build a `WorkingSet` on success (transaction execution starts), or return a `PreExecWorkingSet` on failure (so that we can penalize the sequencer).
        - In `attempt_tx` we consume the `WorkingSet` and output a `TxScratchpad`
        - The `penalize_sequencer` method consumes a `PreExecWorkingSet` and outputs a `TxScratchpad`
  - The `SequencerTxOutcome` type is removed. Now all pre-execution capabilities are processed in `process_tx`. When entering `apply_tx`, the sequencer cannot be penalized anymore.

- #699 adds tests for the `EVM` credentials.
- #717 replaces manual serialization & deserialization for the signed parts of `Transaction` struct by creating a new `UnsignedTransaction` object that implements Borsh traits. The breaking change is that the bytes over we sign now include an additional vector length for the runtime message field.
- #700 adds an associated `TxState` type to the `TxHooks` trait and uses it as an argument in place of a concrete `WorkingSet`.
- #563 introduces a REST API for modules and runtimes. The `RollupBlueprint::create_endpoints` method in `demo-rollup` has been updated accordingly, so it exposes both JSON-RPC and the REST API by default. You can find the documentation for generating a REST API in `sov_modules_api::rest`.
- #696 requires `Runtime` implementers to implement the `HasCapabilities` trait, which allows the `Runtime` to delegate to another struct for much of its required functionality. If a `Runtime` does not wish to delegate, it can simply implement the trait with `Self` as the associated `Capabilities` type. Implementations can be found in the sov-capabailities crate.
- #701 adds support for multiple credentials in `sov-accounts`. This is a breaking change for the consumers of the SDK.
- #694 adds `Credentials` to the `Context` structure. This is a breaking change for the consumers of the SDK. See implementation of the `TransactionAuthorizer::resolve_context` method.
- #688 follow-up of https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/681 that modifies the gas interface to prevent access to `TxGasMeter` outside of `sov-modules-api`.
The spirit of this change is to increase the coupling between the `WorkingSet` and `Transaction` types. Meaningful changes:
  - Change the visibility of `TxGasMeter` to `pub(crate)`.
  - Change the `WorkingSet` interface to return `TransactionConsumption` instead of `GasMeter` when consumed with `revert` or `checkpoint`.
  - Move the `TransactionConsumption` and `SequencerReward` types to `scratchpad`
  - Change the `GasEnforcer` capability to handle the gas workflow without having access to `TxGasMeter`. In particular change the `consume_gas_and_allocate_rewards` capability to the simpler `allocate_consumed_gas` which distributes the transaction reward returned when calling `checkpoint` on the `working_set` to the base fee and tip recipient.
  - Remove access to methods that can artificially modify the gas meter outside of testing (like methods to build `unmetered` gas meters).

- #683 replace `Public Key Hash` with more general concept of `CredentialId`. This is a breaking change for the consumers of the SDK.
- #689 makes `#[rpc_gen]` annotations on modules optional, so you can safely remove it if you don't need it. Additionally, the `get_rpc_methods` function generated by `#[expose_rpc]` now doesn't depend on the import of e.g. `BankRpcImpl` and you should these imports.
- #683 replace `Public Key Hash` with more general concept of `CredentialId`. This is a breaking change for the consumers of the SDK.
- #686 Changes the API of `Module::genesis` to accept an `&mut impl sov_modules_api::GenesisState<S>` instead of a concrete type.
- #679 remove the transaction signature check from the `EVM` module.
- #681 contains some interface improvements for the gas which become possible after `sov-core` and `sov-api` got merged. In particular:
    - Move `TxGasMeter` from `common/gas` to `transaction` which allows more type-safety by removing methods to create and modify arbitrary gas meters, tying it to the `AuthenticatedTransactionData` type.
    - Remove `Tx` and `TxGasMeter` associated types from the `GasEnforcer` trait.
- #647 completes the gas workflow for the StfBlueprint by enhancing the interface by adding some type-safety guarantees to the StfBlueprint and simplifying the penalization workflow. Follow-up of #619.
    - Added a new `consume_gas_and_allocate_rewards` capability to the GasEnforcer to allocate transaction rewards at the end of the transaction execution. This was previously done in `refund_remaining_gas`
    - Added a new type `TransactionConsumption` that tracks the amount of gas consumed and can only be built using the `AuthenticatedTransactionData` and by consuming the associated `TxGasMeter`.
    - `refund_remaining_gas` can only be called either after `consume_gas_and_allocate_rewards` or with a zero `TransactionConsumption` (speculative case for reverted transactions)
    - after `consume_gas_and_allocate_rewards` the `GasMeter` is consumed and cannot be used anymore
- #673 Removes `std` feature from `rollup-interface` and `no_std` support. usage of `sov_rollup_interface::maybestd` should be changed back to `std`.
- #680 Extends sov-cli:
    - Adds new optional boolean parameter to `submit-batch`, that tells sov-cli to wait for batch to be processed by full node
    - set url now expects second parameter for REST API endpoint.
- #663 Modifies the interface of traits `TransactionAuthenticator` and `TransactionAuthorizer`. Associated types `Tx` and `Gas` have been removed. `TransactionAuthenticator` is now generic over `S: Spec`. Methods' type signatures have been slightly modified; please see `examples/demo-rollup/stf/src/authentication.rs` for an example on the new usage.
- #633 Deprecate `sov-modules-core`, move definitions into `sov-modules-api` & `sov-state`
- #664 removes the `Transaction` wrapping in `sov-ethereum` for EVM transactions. This is a breaking change for consumers of the SDK. See `TransactionAuthenticator::authenticate`.
- #646 adds authenticator dispatch logic in`TransactionAuthenticator::authenticate`.
- #613 Makes `sov_state::Storage` trait to be immutable and explicitly produce changes. SimpleStorageManager should be used when data needs to be persisted between batches.
- #631 removes the need for modules to `#[derive(ModuleCallJsonSchema)]`; the trait is automatically blanket-implemented for all modules as long as `CallMessage` implements `schemars::JsonSchema`.
- #628 all the account resolution logic was moved to `resolve_context`. This method now returns a `Result<Context, _ >` instead of a `Context`. This is a breaking change for consumers of the SDK.
- #621 removes the need for a prelude `sov_modules_api::prelude` which re-exposes a few common types for convenience, as well as external crates like `clap` and `serde_json` (for now, more will follow). You can remove these dependencies from your `Cargo.toml` if you wish.
- #620 Adds more fields to the `Event`s emitted by the `sov-bank` module. Start emitting events for token minting.
- #619 starts charging gas for signature checks in the StfBlueprint and completes the refactoring effort started in #612. There was the following changes in the interface:
  - Introduction of a `GasMeter` trait and the three associated implementations: `TxGasMeter` (what used to be the `GasMeter` struct), `UnlimitedGasMeter` (a gas meter that holds an infinite reserve of gas) and the `SequencerStakeMeter` (a gas meter specially designed to track the sequencer stake and accumulate penalties).
  - Adding the `sequencer_stake_meter` as an argument of the `authenticate` method of the `TransactionAuthenticator` (as an associated type) and the `Authenticator` (as a `&mut impl GasMeter` in that case).
  - Adding the `refund_sequencer` capability which can be used to refund the sequencer some of the penalties he accumulated during the pre-execution checks.
  - Modify the `authorize_sequencer` and `penalize_sequencer` capabilities to take the `SequencerGasMeter` as a parameter instead of a fixed amount. This allows type safety and removes the unsafe `saturating_sub` in the implementation of `penalize_sequencer`.
  - Add a `TxSequencerOutcome` which is an enum with the variants `Rewarded(amount)` and `Penalized`.
  - Rename `SequencerOutcome` to `BatchSequencerOutcome` to represent the `SequencerOutcome` following batch execution.

- #622 Make `DefaultStorageSpec` generic over a `Hasher` instead of defaulting to `Sha256`
- #622 Make `DefaultStorageSpec` generic over a `Hasher` instead of defaulting to `Sha256`
- #623 updates the AccountConfig structure. This change is breaking for consumers of the SDK, as the format of the accounts.json configuration files has been changed.
- #617 adds a `gas_estimate` method to the DA layer `Fee` trait
- #614 Change the final argument of `Module::call` from a concrete `WorkingSet` to an abstract `impl TxState` type. It also removes the `working_set.accessory_state()` method and grants direct access to that state through the `WorkingSet` (read/write) and `impl TxState` (read-only).
- #612 refactors the `StfBlueprint` to deserialize, perform signature checks and execute transactions in one pass instead of doing pre-checks for the entire batch followed by the batch execution. Simplifies and fix the sequencer reward workflow. In particular:
  - Replace the previous mechanism that was using a mutable `i64` typed, `sequencer_reward` variable that accumulated the reward/penalties for the sequencer in the entire batch. This mechanism was buggy and did not properly account for side effects (e.g when the sequencer gets penalized more than his current stake, his transactions shouldn't be processed anymore).
  - Augment the authorization error for the `rawTx` verification with an enum with 2 variants: `FatalError` (sequencer gets slashed for acting maliciously) and `Invalid` (sequencer gets penalized a constant amount from his balance).
  - Remove the `Penalized` variant from the `SequencerOutcome`. This is replaced by the `penalize_sequencer` capability that is called whenever the sequencer gets penalized.
  - Add a `authorize_sequencer` capability that verifies that the sequencer has locked enough tokens to execute the next transaction. Since the sequencer bond can vary between transactions (because he may get penalized), this method needs to be called whenever the sequencer start executing a new transaction.
- #599 changes the way you define Rust constants which use the `constants.toml` file.
  Instead of the attribute macro `#[config_constant]`, you'll now use `config_value!`, like this:

  ```rust
  // Before
  #[config_constant]
  pub(crate) const GAS_TX_FIXED_COST: [u64; 2];

  // After
  pub(crate) const GAS_TX_FIXED_COST: [u64; 2] = config_value!("GAS_TX_FIXED_COST");

  // For Bech32:
  // Before
  #[config_bech32_constant]
  const TEST_TOKEN_ID: TokenId;

  // After
  const TEST_TOKEN_ID: TokenId = config_bech32!("TEST_TOKEN_ID", TokenId);
  ```

  Read the PR description for more details.

- #586 Adds a second Zkvm generic to the `StateTransitionFunction` API. This VM is used for generation of *`Aggregate`* zk proofs,
while the first VM continues Ato be used for block production. The signature of the `StfVerifier<DA, Vm, ZkSpec, RT, K>` was also changed to `StfVerifier<DA ZkSpec, RT, K, InnerVm, OuterVm>`

- #584 removes support for the `DefaultRuntime` derive macro. You must replace all proc-macro invocations of `DefaultRuntime` with `#[derive(Default)]`.
- #590 upgrade rustc version to 1.77. Installation new risc0 toolchain is needed. Simply
  run `make install-risc0-toolchain`
- #584 removes support for the `DefaultRuntime` derive macro. You must replace all proc-macro invocations
  of `DefaultRuntime` with `#[derive(Default)]`.

- #572 adds a new `da_service` method `estimate_fee()` and requires all blob submission methods to provide a `fee` as an argument.
- #580 `sov-cli` now returns an error on duplicate nickname key import or generation.
- #572 adds a new `da_service` method `estimate_fee()` and requires all blob submission methods to provide a `fee` as an
  argument.
- #573 Changes the `Bank::create_token signature`, allowing modules to create tokens.

- #547 removes the `From<[u8;32]>` bound on rollup addresses and replaces it with a bound expressing that for
  any `Spec`, the `Address` type must implement `From<&PublicKey>`. It also removes the nft module's `CollectionAddress`
  type and replaces it with a 32-byte `CollectionId`.
- #546 fixes a bug where sequencer could protect themselves from slashing by exiting while their batch was being
  processed.

- #545 moves the `RelevantBlobs`, `RelevantBlobIters`, and `DaProof` types from `rollup_interfaces::da::services`
  to `rollup_interfaces::da`. It also adds a feature gate to require the `native` flag to
  use `rollup_interfaces::da::services`.

- #535 fixes the sequencer tip calculation in the `StfBlueprint`. This calculation now takes into account the changes of
  the #429 (which fixes the gas reserve calculation). Also renames `max_priority_fee` to `max_priority_fee_bips` in
  the `Transaction` structure.

- #528 removes the `initial_base_fee_per_gas` parameter from the genesis configuration of the chain state to define a
  constant `INITIAL_BASE_FEE_PER_GAS` that is common to all crates. Now that we assume the tx sender always has a bank
  account for the gas token, the non-zero transfer amount hacks from the `reserve_gas` and `refund_remaining_gas`
  capabilities for the `GasEnforcer` in the `Bank` module are replaced by an account check. If the sender doesn't have a
  bank account for the gas token, the method fails with the error `AccountDoesNotExist`. This check is done in the
  EIP-1559 specification.
- #519 Adds an `Authenticator` trait which abstracts away the transaction authentication logic. This is a breaking
  change so the consumers of the SDK will need to implement the new trait.
  See `authentication.rs` in `demo-rollup`.

- #538 removes the chain state from the EVM module because it was unused.

- #528 removes the `initial_base_fee_per_gas` parameter from the genesis configuration of the chain state to define a
  constant `INITIAL_BASE_FEE_PER_GAS` that is common to all crates. Now that we assume the tx sender always has a bank
  account for the gas token, the non-zero transfer amount hacks from the `reserve_gas` and `refund_remaining_gas`
  capabilities for the `GasEnforcer` in the `Bank` module are replaced by an account check. If the sender doesn't have a
  bank account for the gas token, the method fails with the error `AccountDoesNotExist`. This check is done in the
  EIP-1559 specification.

- #526 `Bank::create_token` & `Bank::mint` now accept an `impl Payable` as arguments. This is a breaking
  change. `&S::Address` already implements `Payable`, and `ModuleId` can be promoted to `Payable`
  via `ModuleId::as_token_holder`.

- #525 mitigates the bug where a transaction which was included multiple times would always return the `Duplicate` tx
  status rather than returning info about the original tx execution. In this commit we use a simple heuristic (first tx
  wins) to guess which instance is the "correct" one.


- #468 fixes the gas elasticity computation that was removed in #476. The base fee computation is now done in
  the `ChainState` module as part of the `begin_slot` hook. This PR also updates the `ChainState` integration tests to
  check that the base fee computation is correctly performed in the module hooks. It also adds gas elasticity tests for
  the `ChainState` module.

- #490 Adds a `inner_code_commitment`, an `outer_code_commitment` and a `initial_da_height` field to the `ChainState`
  module. These fields should be initialized at genesis. Added getters for `genesis_da_height`, `outer_code_commitment`
  and `inner_code_commitment` in the `ChainState` module. Adapted the `demo-rollup` json configurations to use these
  fields. Added a new configuration folder for the stf tests that rely on `MockCodeCommitment` which have a different
  format from the `risc0` code commitments (32 bytes instead of 8). Modified the `AttesterIncentives` module to use
  the `inner_code_commitment` field from the `ChainState` module instead of
  the `commitment_to_allowed_challenger_method` field. Modified the `ProverIncentives` module to use
  the `outer_code_commitment` field from the `ChainState` module instead of the `commitment_of_allowed_verifier_method`
  field.

- #487 Introduces the `AuthenticatedTransactionData` structure. This is the transaction data that passed the
  authentication phase. And updates `Accounts::CallMessage` format. This is a breaking change for consumers of the SDK
  only if they send messages directly to the Accounts module.

- #484 Adds a new `CodeCommitment` trait and applies it to the associated type of the ZKVM. The
  trait provides encode/decode methods, in addition to all existing functionality. These methods
  should be used to convert to/from the `code_commitment` vector in `AggregatedProofPublicData`.

- #480 The `Accounts` module now keeps PublicKey hashes instead of PublicKeys. This is a breaking change for consumers
  of the SDK only if they send messages directly to the Accounts module.

- #479 refactors the `ChainState` module integration test to be more readable and less repetitive.

- #476 updates the gas interface for the ChainState module, removes the gas price elasticity computation (it will be
  fixed in #468) and propagates these changes throughout the infrastructure.
  Meaningful changes:
    - Added the `INITIAL_GAS_LIMIT` and `initial_gas_price` (defined at genesis) constants. These constants are defined
      in the EIP-1559 and are used to handle the gas lifecycle in the chain-state module
    - Rename the `gas_price` (a generic name not used anywhere in the EIP-1559) to `base_fee_per_gas` which is the
      official naming for this variable
    - Create a `BlockGasInfo` structure that groups the `gas_used`, `gas_limit` and `base_fee_per_gas` into one wrapper.
    - Removed the `gas_price_state` from the `chain-state` module's state. There was multiple reasons behind that:
    - Removed the outdated gas elasticity mechanism

- #481 This PR combines the `ContextResolver` and `TransactionDeduplicator` traits into a single `TransactionAuthorizer`
  trait. This is a breaking change, and consumers of the SDK will need to implement the new trait.

- #472 This PR breaks downstream code in the following way:
  `PublicKey::to_address` is now parameterized by `Hasher`.

- #471 adds 3 new parameters to sov-demo-rollup
    - optional cmd `--genesis-config-dir ../test-data/genesis/demo/celestia` to specify the genesis config directory
    - optional cmd `--prometheus_exporter_bind 127.0.0.1:9845` to specify the prometheus exporter bind address. Useful
      for running several nodes on the same host.
    - environment `export SOV_TX_SIGNER_PRIV_KEY_PATH=examples/test-data/keys/tx_signer_private_key.json` to specify the
      path to the transaction signer private key.

- #452 abstracts away the transaction authorization logic. The consumers of the `sov-module-api` have to implement the
  new `TransactionAuthenticator` trait. Refer to `hooks_impl.rs` for details

- #413 introduces new RESTful JSON APIs for the sequencer and, most importantly, modifies the `RollupBlueprint` trait
  interface to allow implementations to expose Axum servers, instead of only JSON-RPC servers. In
  fact, `RollupBlueprint::create_rpc_methods` was renamed to `RollupBlueprint::create_endpoints`, which returns a tuple.
  Most `RollupBlueprint` implementations will need to use the new `sov_modules_rollup_blueprint::register_endpoints`,
  which replaces `sov_modules_rollup_blueprint::register_rpc`. Take a look at how `examples/demo-rollup` implements the
  new interface to see how it works.

- #439 Implements the `SequencerRegistry` module to support Sequencers' reward and penalties. In particular,
  the `SequencerRegistry` can now be used in conjunction with the `GasEnforcer` capability hook to reward the
  sequencer for submitting a correct transaction.

- #444 Moves the tests for the `SequencerRegistry` module to the `src` directory of the same crate.

- #443 Removes the `coins` field in the `SequencerRegistry` struct. It is replaced by a `minimum_bond` field and
  the `TokenId` becomes `GAS_TOKEN_ID`. The configuration structure `SequencerRegistryConfig` should be updated to
  replace the `coin` field by the new `minimum_bond` field.

- #432 Updates the `StateTransitionFunction`  to handle blobs from all the relevant namespaces.
  This breaks the `StateTransitionFunction` API but the breaking changes don't propagate outside of the module system
  internals.

- #441 Removes the section of the `rollup_cofing.toml` called `[prover_service]` and moves its existing value to a
  section called `[proof_manager]`. To update, it's sufficient to simply search and replace `[prover_service]`
  to `[proof_manager]` in any configuration files.

- #429 Updates the `reserve_gas` and `refund_remaining_gas` mechanisms to match EIP-1559. The `reserve_gas`
  and `refund_remaining_gas` methods are moved back to the `Bank` module as they now affect multiple modules (the module
  that locks the gas tip - ie the `sequencer-registry` - and the module that locks the base gas - ie
  the `attester-incentives` or `prover-incentives`). Instead of locking the gas in
  the `attester-incentives`, `prover-incentives` or `sequencer-registry` at the `reserve_gas` call, we are now doing it
  when `refund_remaining_gas` is called. The `Transaction` structure is updated to let the user specify a `max_fee` and
  a `max_priority_fee` which are respectively a coin amount and a percentage. He may optionally specify a `gas_limit`
  which is a multi-dimensional gas limit that is used as a protection for gas elasticity (following EIP-1559).

- #425 Updates the `CelestiaVerifier` to support multiple namespaces. This change is breaking for consumers of
  the `Sovereign-sdk`: The `CelestiaVerifier` now needs to be initialized with `ROLLUP_BATCH_NAMESPACE`
  and `ROLLUP_PROOF_NAMESPACE`. See:
    1. https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/425/files#diff-75e27b2869f342897e1c89ed4abe7ff82ce8368a795dbefdffac8e30bbcb11f4L36

    2. https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/425/files#diff-d46bdfc6e8e6dfb4acd9794c4536d6a8212b37aef27abc4b39d7db479be75d4aL135

- #406 Updates the `DaService` trait and the `Celestia` adapter to support multiple namespaces. This changes are
  transparent to the `RollupBlueprint`.

- #361 starts charging gas for submitting transactions to the Rollup. When calling `apply_slot`, the transaction sender
  must pay for a fixed amount of gas - `GAS_TX_FIXED_COST`. Developers have to make sure the transaction sender has
  enough funds to pay for the gas.

- #385 makes the `reward_burn_rate` constant in the `ProverIncentives` module and transforms the associated getters to
  be infallible. In the future, the reward burn rate will have to be set in the `constants.toml` and
  the `constants.testing.toml` files and need to be a value in the range [0, 99].

- #340 moves the Kernels' implementation (currently the `BasicKernel` and the `SoftConfirmationsKernel`) to a
  dedicated `sov-kernel` crate.

- #347 renames the following types:
  `StateTransitionData` to `StateTransitionWitness`
  `StateTransition` to `StateTransitionPublicData`
  `AggregatedProofPublicInput` to `AggregatedProofPublicData`

- #329 adds `InnerZkvm` and `OuterZkvm` associated types to the `Spec` trait.

- #306 removes the `State*Accessor` traits and replaces them with methods on (Acessory)StateValue/Map types. You can
  simply remove
  any imports of these traits and the `sov_modules_api::prelude*`. Also simplifies the API of VersionedStateValue. Now
  it only has a method `get_current` (for any type implementing the `VersionReader` trait)
  and get/set implemented directly on `KernelStateAccessor`

- #266 implements reward/slashing mechanisms for provers in the `ProverIncentives` module. In particular, given that an
  aggregated proof can be correctly serialized and the proof outputs are corrects, the provers can be rewarded for the
  new block transitions they proved. If no new block transitions are proved as part of the aggregated proof, then the
  prover is penalized by a fixed amount.
  The prover may be slashed if it posts an invalid proof or a proof for a state transition that doesn't exist.

- #170 unifies `CacheKey/Value` and `StorageKey/Value` data structures into `SlotKey/Value` data structures.

- #253 adds block validity conditions as part of aggregated proofs public inputs. This then can ensure that the validity
  conditions are stored on-chain for out-of-circuit verification. The validity conditions are stored as a `Vec<u8>`,
  after being serialized using `Borsh`.

- #242 changes the behavior of the `AttesterIncentives` module to gracefully exit when users are slashed and the state
  gets updated. The slashing reason can be retrieved as part of the `UserSlashed` event that gets emitted. Also contains
  small changes to the traits derived by the structures contained in the module, so that the module can be included in
  the runtime structures. We also add the `Checker` associated type to the `DaSpec` trait which considerably simplifies
  the module structure definition (contains two generics instead of 4)

- #169 achieves the rollup state separation in different namespaces. Conceptually, each namespace is just defined by a
  triple of tables inside a shared state db - there is only one `StateDb`.
- #451 Removes optional transactions list from RPC endpoints `eth_publishBatch`.
