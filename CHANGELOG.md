- #306 removes the `State*Accessor` traits and replaces them with methods on (Acessory)StateValue/Map types. You can simply remove
  any imports of these traits and the `sov_modules_api::prelude*`.

- #306 simplifies the API of VersionedStateValue. Now it only has a method `get_current` (for any type implementing the `VersionReader` trait)
  and get/set implemented directly on `KernelWorkingSet`

- #329 adds `InnerZkvm` and `OuterZkvm` associated types to the `Spec` trait.

- #347 renames the following types:
`StateTransitionData` to `StateTransitionWitness`
`StateTransition` to `StateTransitionPublicData`
`AggregatedProofPublicInput` to `AggregatedProofPublicData`
   
