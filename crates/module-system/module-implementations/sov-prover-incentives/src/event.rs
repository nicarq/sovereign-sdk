#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Clone,
)]
/// Reasons for slashing a prover
pub enum SlashingReason {
    /// The proof is not a valid zk-proof - ie the verifier did not accept the proof.
    ProofInvalid,

    /// The genesis hash supplied is incorrect
    IncorrectGenesisHash,

    /// The initial state root contained in the [`sov_modules_api::AggregatedStateTransition`] outputs is incorrect
    IncorrectInitialStateRoot,

    /// The initial transition slot contained in the [`sov_modules_api::AggregatedStateTransition`] has no associated transition
    /// in the chain state module.
    InitialTransitionDoesNotExist,

    /// The initial slot hash contained in the [`sov_modules_api::AggregatedStateTransition`] outputs is incorrect
    IncorrectInitialSlotHash,

    /// The final transition slot contained in the [`sov_modules_api::AggregatedStateTransition`] has no associated transition
    /// in the chain state module.
    FinalTransitionDoesNotExist,

    /// The final state root contained in the [`sov_modules_api::AggregatedStateTransition`] outputs is incorrect
    IncorrectFinalStateRoot,

    /// The final slot hash contained in the [`sov_modules_api::AggregatedStateTransition`] outputs is incorrect
    IncorrectFinalSlotHash,

    /// The initial slot hash contained in the [`sov_modules_api::AggregatedStateTransition`] outputs is incorrect
    IncorrectValidityConditions,
}

#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Clone,
)]
/// The reasons for penalizing a prover
pub enum PenalizationReason {
    /// We penalize the prover for submitting a proof for transitions that have already been processed
    ProofAlreadyProcessed,
}

#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Clone,
)]
/// Events for prover incentives
pub enum Event<S: sov_modules_api::Spec> {
    /// The prover has been bonded. The deposit is the amount of the bond and the total balance is the total amount staked.
    BondedProver {
        /// The address of the prover that was bonded.
        prover: S::Address,
        /// The amount deposited by the prover for bond.
        deposit: u64,
        /// The total amount bonded for the prover.
        total_balance: u64,
    },
    /// The prover has been unbonded. The amount withdrawn is the amount of the bond that was withdrawn.
    UnBondedProver {
        /// The address of the prover that was unbonded.
        prover: S::Address,
        /// The amount that was withdrawn from the provers bond.
        amount_withdrawn: u64,
    },
    /// The prover has been slashed. The reason describes why the prover was slashed.
    ProverSlashed {
        /// The address of the prover that was slashed.
        prover: S::Address,
        /// The reason the prover was slashed.
        reason: SlashingReason,
    },
    /// The prover has been penalized (fined). The reason describes why the prover was fined.
    ProverPenalized {
        /// The address of the prover that was penalized.
        prover: S::Address,
        /// The amount the prover was penalized, this is taken from their bond.
        amount: u64,
        /// The reason the prover was penalized.
        reason: PenalizationReason,
    },
    /// Event for processing a valid proof
    ProcessedValidProof {
        /// The address of the prover that submitted a proof that was processed and determined to
        /// be valid.
        prover: S::Address,
        /// The amount the prover was rewarded for submitting a valid proof.
        reward: u64,
    },
}
