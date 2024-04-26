- #538 removes the chain state from the EVM module because it was unused.

- #528 removes the `initial_base_fee_per_gas` parameter from the genesis configuration of the chain state to define a constant `INITIAL_BASE_FEE_PER_GAS` that is common to all crates. Now that we assume the tx sender always has a bank account for the gas token, the non-zero transfer amount hacks from the `reserve_gas` and `refund_remaining_gas` capabilities for the `GasEnforcer` in the `Bank` module are replaced by an account check. If the sender doesn't have a bank account for the gas token, the method fails with the error `AccountDoesNotExist`. This check is done in the EIP-1559 specification.
- #526 `Bank::create_token` & `Bank::mint` now accept an `impl Payable` as arguments. This is a breaking change. `&S::Address` already implements `Payable`, and `ModuleId` can be promoted to `Payable` via `ModuleId::as_token_holder`.

- #525 mitigates the bug where a transaction which was included multiple times would always return the `Duplicate` tx status rather than returning info about the original tx execution. In this commit we use a simple heuristic (first tx wins) to guess which instance is the "correct" one.


- #468 fixes the gas elasticity computation that was removed in #476. The base fee computation is now done in the `ChainState` module as part of the `begin_slot` hook. This PR also updates the `ChainState` integration tests to check that the base fee computation is correctly performed in the module hooks. It also adds gas elasticity tests for the `ChainState` module.

- #490 Adds a `inner_code_commitment`, an `outer_code_commitment` and a `initial_da_height` field to the `ChainState` module. These fields should be initialized at genesis. Added getters for `genesis_da_height`, `outer_code_commitment` and `inner_code_commitment` in the `ChainState` module. Adapted the `demo-rollup` json configurations to use these fields. Added a new configuration folder for the stf tests that rely on `MockCodeCommitment` which have a different format from the `risc0` code commitments (32 bytes instead of 8). Modified the `AttesterIncentives` module to use the `inner_code_commitment` field from the `ChainState` module instead of the `commitment_to_allowed_challenger_method` field. Modified the `ProverIncentives` module to use the `outer_code_commitment` field from the `ChainState` module instead of the `commitment_of_allowed_verifier_method` field. 

- #487 Introduces the `AuthenticatedTransactionData` structure. This is the transaction data that passed the authentication phase. And updates `Accounts::CallMessage` format. This is a breaking change for consumers of the SDK only if they send messages directly to the Accounts module.

- #484 Adds a new `CodeCommitment` trait and applies it to the associated type of the ZKVM. The 
trait provides encode/decode methods, in addition to all existing functionality. These methods 
should be used to convert to/from the `code_commitment` vector in `AggregatedProofPublicData`. 

- #480 The `Accounts` module now keeps PublicKey hashes instead of PublicKeys. This is a breaking change for consumers of the SDK only if they send messages directly to the Accounts module.

- #479 refactors the `ChainState` module integration test to be more readable and less repetitive. 

- #476 updates the gas interface for the ChainState module, removes the gas price elasticity computation (it will be fixed in #468) and propagates these changes throughout the infrastructure.
Meaningful changes:
  - Added the `INITIAL_GAS_LIMIT` and `initial_gas_price` (defined at genesis) constants. These constants are defined in the EIP-1559 and are used to handle the gas lifecycle in the chain-state module
  - Rename the `gas_price` (a generic name not used anywhere in the EIP-1559) to `base_fee_per_gas` which is the official naming for this variable
  - Create a `BlockGasInfo` structure that groups the `gas_used`, `gas_limit` and `base_fee_per_gas` into one wrapper.
  - Removed the `gas_price_state` from the `chain-state` module's state. There was multiple reasons behind that:
  - Removed the outdated gas elasticity mechanism

- #481 This PR combines the `ContextResolver` and `TransactionDeduplicator` traits into a single `RuntimeAuthorization` trait. This is a breaking change, and consumers of the SDK will need to implement the new trait.

- #472  This PR breaks downstream code in the following way:
  `PublicKey::to_address` is now parameterized by `Hasher`.

- #471 adds 3 new parameters to sov-demo-rollup
  - optional cmd `--genesis-config-dir ../test-data/genesis/demo/celestia` to specify the genesis config directory
  - optional cmd `--prometheus_exporter_bind 127.0.0.1:9845` to specify the prometheus exporter bind address. Useful for running several nodes on the same host.
  - environment `export SOV_TX_SIGNER_PRIV_KEY_PATH=examples/test-data/keys/tx_signer_private_key.json` to specify the path to the transaction signer private key.

- #452  abstracts away the transaction authorization logic. The consumers of the `sov-module-api` have to implement the new `RuntimeAuthenticator` trait. Refer to `hooks_impl.rs` for details

- #413 introduces new RESTful JSON APIs for the sequencer and, most importantly, modifies the `RollupBlueprint` trait interface to allow implementations to expose Axum servers, instead of only JSON-RPC servers. In fact, `RollupBlueprint::create_rpc_methods` was renamed to `RollupBlueprint::create_endpoints`, which returns a tuple. Most `RollupBlueprint` implementations will need to use the new `sov_modules_rollup_blueprint::register_endpoints`, which replaces `sov_modules_rollup_blueprint::register_rpc`. Take a look at how `examples/demo-rollup` implements the new interface to see how it works.

- #439 Implements the `SequencerRegistry` module to support Sequencers' reward and penalties. In particular,
the `SequencerRegistry` can now be used in conjunction with the `GasEnforcer` capability hook to reward the 
sequencer for submitting a correct transaction.

- #444 Moves the tests for the `SequencerRegistry` module to the `src` directory of the same crate.

- #443 Removes the `coins` field in the `SequencerRegistry` struct. It is replaced by a `minimum_bond` field and the `TokenId` becomes `GAS_TOKEN_ID`. The configuration structure `SequencerRegistryConfig` should be updated to replace the `coin` field by the new `minimum_bond` field.

- #432 Updates the `StateTransitionFunction`  to handle blobs from all the relevant namespaces.
This breaks the `StateTransitionFunction` API but the breaking changes don't propagate outside of the module system internals. 

- #441 Removes the section of the `rollup_cofing.toml` called `[prover_service]` and moves its existing value to a section called `[proof_manager]`. To update, it's sufficient to simply search and replace `[prover_service]` to `[proof_manager]` in any configuration files.

- #429 Updates the `reserve_gas` and `refund_remaining_gas` mechanisms to match EIP-1559. The `reserve_gas` and `refund_remaining_gas` methods are moved back to the `Bank` module as they now affect multiple modules (the module that locks the gas tip - ie the `sequencer-registry` - and the module that locks the base gas - ie the `attester-incentives` or `prover-incentives`). Instead of locking the gas in the `attester-incentives`, `prover-incentives` or `sequencer-registry` at the `reserve_gas` call, we are now doing it when `refund_remaining_gas` is called. The `Transaction` structure is updated to let the user specify a `max_fee` and a `max_priority_fee` which are respectively a coin amount and a percentage. He may optionally specify a `gas_limit` which is a multi-dimensional gas limit that is used as a protection for gas elasticity (following EIP-1559).

- #425 Updates the `CelestiaVerifier` to support multiple namespaces. This change is breaking for consumers of the `Sovereign-sdk`: The `CelestiaVerifier` now needs to be initialized with `ROLLUP_BATCH_NAMESPACE` and `ROLLUP_PROOF_NAMESPACE`. See:
  1. https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/425/files#diff-75e27b2869f342897e1c89ed4abe7ff82ce8368a795dbefdffac8e30bbcb11f4L36

  2. https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/425/files#diff-d46bdfc6e8e6dfb4acd9794c4536d6a8212b37aef27abc4b39d7db479be75d4aL135

- #406 Updates the `DaService` trait and the `Celestia` adapter to support multiple namespaces. This changes are transparent to the `RollupBlueprint`.

- #361 starts charging gas for submitting transactions to the Rollup. When calling `apply_slot`, the transaction sender must pay for a fixed amount of gas - `GAS_TX_FIXED_COST`. Developers have to make sure the transaction sender has enough funds to pay for the gas.

- #385 makes the `reward_burn_rate` constant in the `ProverIncentives` module and transforms the associated getters to be infaillible. In the future, the reward burn rate will have to be set in the `constants.json` and the `constants.test.json` files and need to be a value in the range [0, 99].

- #340 moves the Kernels' implementation (currently the `BasicKernel` and the `SoftConfirmationsKernel`) to a dedicated `sov-kernel` crate.

- #347 renames the following types:
  `StateTransitionData` to `StateTransitionWitness`
  `StateTransition` to `StateTransitionPublicData`
  `AggregatedProofPublicInput` to `AggregatedProofPublicData`

- #329 adds `InnerZkvm` and `OuterZkvm` associated types to the `Spec` trait.

- #306 removes the `State*Accessor` traits and replaces them with methods on (Acessory)StateValue/Map types. You can simply remove
  any imports of these traits and the `sov_modules_api::prelude*`. Also simplifies the API of VersionedStateValue. Now it only has a method `get_current` (for any type implementing the `VersionReader` trait)
  and get/set implemented directly on `KernelWorkingSet`

- #266 implements reward/slashing mechanisms for provers in the `ProverIncentives` module. In particular, given that an aggregated proof can be correctly serialized and the proof outputs are corrects, the provers can be rewarded for the new block transitions they proved. If no new block transitions are proved as part of the aggregated proof, then the prover is penalized by a fixed amount.
The prover may be slashed if it posts an invalid proof or a proof for a state transition that doesn't exist.

- #170 unifies `CacheKey/Value` and `StorageKey/Value` data structures into `SlotKey/Value` data structures.

- #253 adds block validity conditions as part of aggregated proofs public inputs. This then can ensure that the validity conditions are stored on-chain for out-of-circuit verification. The validity conditions are stored as a `Vec<u8>`, after being serialized using `Borsh`.

- #242 changes the behavior of the `AttesterIncentives` module to gracefully exit when users are slashed and the state gets updated. The slashing reason can be retrieved as part of the `UserSlashed` event that gets emitted. Also contains small changes to the traits derived by the structures contained in the module, so that the module can be included in the runtime structures. We also add the `Checker` associated type to the `DaSpec` trait which considerably simplifies the module structure definition (contains two generics instead of 4)

- #169 achieves the rollup state separation in different namespaces. Conceptually, each namespace is just defined by a triple of tables inside a shared state db - there is only one `StateDb`.
- #451 Removes optional transactions list from RPC endpoints `eth_publishBatch`.
