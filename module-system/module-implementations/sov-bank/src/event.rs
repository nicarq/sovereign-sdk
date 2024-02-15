/// Bank Event
#[derive(borsh::BorshDeserialize, borsh::BorshSerialize, Debug, PartialEq, Clone)]
#[cfg_attr(feature = "native", derive(serde::Serialize, serde::Deserialize))]
pub enum Event<C: sov_modules_api::Context> {
    /// Event for Token Creation
    TokenCreated {
        /// The address of the token that has been created
        token_address: C::Address,
    },
    /// Event for Token Transfer
    TokenTransferred {
        /// The address of the token that was transferred
        token_address: C::Address,
        /// The quantity of the token that was transferred
        amount: u64,
    },
}
