/// Template Event
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
    /// Template event set value
    Set { value: u32 },
}
