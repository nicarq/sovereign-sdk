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
pub enum Event {
    /// Push
    Push { value: u32, length: usize },
    /// Set
    Set { value: u32, index: usize },
    /// SetAll
    SetAll { length: usize },
    /// Pop
    Pop {
        pop_value: Option<u32>,
        length: usize,
    },
}
