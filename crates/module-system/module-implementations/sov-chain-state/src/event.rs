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
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// Sample event variant 1
    Event1,
    /// Sample event variant 2
    Event2,
}
