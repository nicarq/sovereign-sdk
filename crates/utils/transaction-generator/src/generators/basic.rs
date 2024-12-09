use derivative::Derivative;
use sov_modules_api::Spec;

use super::bank::{BankChangeLogEntry, BankMessageGenerator, BankTag};
use super::factory::CallMessageFactory;
use super::value_setter::{ValueSetterChangeLogEntry, ValueSetterMessageGenerator, ValueSetterTag};
use crate::CallMessageGenerator;

/// A basic call message generator factory that can be used with modules internal to the sovereign sdk
pub type BasicCallMessageFactory<RT, S, Acct = ()> =
    CallMessageFactory<RT, S, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig, Acct>;

/// The set of tags supported by the [`BasicCallMessageFactory`].
#[derive(Clone, Copy, Derivative, Debug, derive_more::From)]
#[derivative(PartialEq, Eq, Hash)]
pub enum BasicTag {
    /// Tags for the bank module
    Bank(BankTag),
    /// Tags for the value setter module
    ValueSetter(ValueSetterTag),
}

/// The set of change log entries supported by the [`BasicCallMessageFactory`].
#[derive(Clone, Debug, strum::EnumDiscriminants, derive_more::From)]
#[strum_discriminants(name(SupportedModules))]
pub enum BasicChangeLogEntry<S: Spec> {
    /// Changes from the bank module
    Bank(BankChangeLogEntry<S>),
    /// Changes from the value setter module
    ValueSetter(ValueSetterChangeLogEntry),
}

impl<S: Spec> BasicChangeLogEntry<S> {
    /// Asserts the changelog results against the state
    pub async fn assert_against_state(
        &self,
        bank_generator: &BankMessageGenerator<S>,
        value_setter_generator: &ValueSetterMessageGenerator<S>,
        config: &BasicClientConfig,
    ) -> anyhow::Result<()> {
        match self {
            BasicChangeLogEntry::Bank(bank_changelog_entry) => {
                bank_generator
                    .assert_state(config.clone().into(), bank_changelog_entry)
                    .await
            }
            BasicChangeLogEntry::ValueSetter(value_setter_changelog_entry) => {
                value_setter_generator
                    .assert_state(config.clone().into(), value_setter_changelog_entry)
                    .await
            }
        }
    }
}

/// The basic configuration for any rollup http client.
#[derive(Debug, Clone)]
pub struct BasicClientConfig {
    /// The url to query.
    pub url: String,
    /// The rollup height to query, if necessary.
    pub rollup_height: Option<u64>,
}
