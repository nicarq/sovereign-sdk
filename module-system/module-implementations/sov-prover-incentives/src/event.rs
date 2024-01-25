/// Events for prover incentives
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Clone,
)]
pub enum Event<C: sov_modules_api::Context> {
    /// Event for Bonded Prover
    BondedProver { deposit: u64, total_balance: u64 },
    /// Event for Unbonded Prover
    UnBondedProver { amount_withdrawn: u64 },
    /// Event for an invalid proof processed resulting in a slashed prover
    ProcessedInvalidProof { slashed_prover: C::Address },
    /// Event for processing a valid proof
    ProcessedValidProof { prover: C::Address },
}
