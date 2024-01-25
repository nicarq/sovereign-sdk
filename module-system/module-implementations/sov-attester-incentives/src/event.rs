/// Events for attester incentives
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
    /// Event for User Slashed
    UserSlashed { address: C::Address },
    /// Event for a new deposit
    BondedAttester { new_deposit: u64, total_bond: u64 },
    /// Event for a new deposit
    BondedChallenger { new_deposit: u64, total_bond: u64 },
    /// Event for a new deposit
    NewDeposit { new_deposit: u64, total_bond: u64 },
    /// Event for Unbonding
    UnbondedChallenger { amount_withdrawn: u64 },
    /// Event for processing a valid attestation
    ProcessedValidAttestation { attester: C::Address },
    /// Event for processing a valid proof
    ProcessedValidProof { challenger: C::Address },
}
