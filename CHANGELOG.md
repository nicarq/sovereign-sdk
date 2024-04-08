- #306 removes the `State*Accessor` traits and replaces them with methods on (Acessory)StateValue/Map types. You can simply remove
  any imports of these traits and the `sov_modules_api::prelude*`.

- #306 simplifies the API of VersionedStateValue. Now it only has a method `get_current` (for any type implementing the `VersionReader` trait)
  and get/set implemented directly on `KernelWorkingSet`

- #329 adds `InnerZkvm` and `OuterZkvm` associated types to the `Spec` trait.

- #340 moves the Kernels' implementation (currently the `BasicKernel` and the `SoftConfirmationsKernel`) to a dedicated `sov-kernel` crate.

- #347 renames the following types:
  `StateTransitionData` to `StateTransitionWitness`
  `StateTransition` to `StateTransitionPublicData`
  `AggregatedProofPublicInput` to `AggregatedProofPublicData`

- #361 starts charging gas for submitting transactions to the Rollup. When calling `apply_slot`, the transaction sender must pay for a fixed amount of gas - `GAS_TX_FIXED_COST`. Developers have to make sure the transaction sender has enough funds to pay for the gas.

- #406 Updates the `DaService` trait and the `Celestia` adapter to support multiple namespaces. This changes are transparent to the `RollupBlueprint`.

- #425 Updates the `CelestiaVerifier` to support multiple namespaces. This change is breaking for consumers of the `Sovereign-sdk`: The `CelestiaVerifier` now needs to be initialized with `ROLLUP_BATCH_NAMESPACE` and `ROLLUP_PROOF_NAMESPACE`. See:
  1. https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/425/files#diff-75e27b2869f342897e1c89ed4abe7ff82ce8368a795dbefdffac8e30bbcb11f4L36

  2. https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/425/files#diff-d46bdfc6e8e6dfb4acd9794c4536d6a8212b37aef27abc4b39d7db479be75d4aL135

