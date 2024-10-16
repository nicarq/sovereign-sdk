use borsh::{BorshDeserialize, BorshSerialize};
use schemars::JsonSchema;
use sov_modules_api::{DaSpec, Spec};

/// Template Event
#[derive(
    serde::Serialize,
    serde::Deserialize,
    BorshDeserialize,
    BorshSerialize,
    Debug,
    PartialEq,
    Clone,
    JsonSchema,
)]
#[serde(bound = "S: Spec", rename_all = "snake_case")]
#[schemars(bound = "S: Spec", rename = "Event")]
pub enum Event<S: Spec> {
    RegisteredPaymaster {
        address: S::Address,
    },
    SetPayerForSequencer {
        sequencer: <S::Da as DaSpec>::Address,
        payer: S::Address,
    },
}
