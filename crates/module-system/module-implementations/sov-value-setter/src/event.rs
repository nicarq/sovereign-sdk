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
    /// New Value event
    NewValue(u32),
}
