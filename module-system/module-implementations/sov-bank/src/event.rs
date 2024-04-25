use crate::TokenId;

/// Bank Event
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Clone,
)]
// TODO - <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/324>
pub enum Event {
    /// Event for Token Creation
    TokenCreated {
        /// The ID of the token that has been created
        token_id: TokenId,
    },
    /// Event for Token Transfer
    TokenTransferred {
        /// The ID of the token that was transferred
        token_id: TokenId,
        /// The quantity of the token that was transferred
        amount: u64,
    },
    /// Some tokens were burned
    TokenBurned {
        /// The ID of the token that was transferred
        token_id: TokenId,
        /// The quantity of the token that was transferred
        amount: u64,
    },
    /// The supply of a token was frozen
    TokenFrozen {
        /// The ID of the token that was transferred
        token_id: TokenId,
    },
}
