use crate::SlashingReason;

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
#[serde(rename_all = "snake_case")]
pub enum Event<S: sov_modules_api::Spec> {
    /// Event for User Slashed
    UserSlashed {
        /// The address of the user who was slashed.
        address: S::Address,
        /// The reason the user was slashed.
        reason: SlashingReason,
    },
    /// Event for registration of a new attester.
    RegisteredAttester {
        /// The amount of tokens deposited by this call.
        amount: u64,
    },

    /// Event for registration of a new challenger.
    RegisteredChallenger {
        /// The amount of tokens deposited by this call.
        amount: u64,
    },

    /// Event for exiting an attester.
    ExitedAttester {
        /// The number of tokens returned to the caller's bank balance.
        amount_withdrawn: u64,
    },
    /// Event for a new deposit.
    BondedChallenger {
        /// The amount of tokens deposited by this call.
        new_deposit: u64,
        /// The total bond of the challenger after this call.
        total_bond: u64,
    },
    /// Event for a new deposit
    NewDeposit {
        /// The amount of tokens deposited by this call.
        new_deposit: u64,
        /// The total bond of the challenger after this call.
        total_bond: u64,
    },
    /// Event for exiting a challenger.
    ExitedChallenger {
        /// The number of tokens returned to the caller's bank balance.
        amount_withdrawn: u64,
    },
}
