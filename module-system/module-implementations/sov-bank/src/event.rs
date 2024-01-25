/// Sample Event
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
    /// Event for Token Creation
    TokenCreated { token_address: C::Address },
    /// Event for Token Transfer
    TokenTransferred { token_address: C::Address },
}
