use std::collections::HashSet;

use derivative::Derivative;
use sov_modules_api::Spec;

use super::bank::{BankChangeLogDiscriminant, BankChangeLogEntry, BankMessageGenerator, BankTag};
use super::factory::CallMessageFactory;
use super::value_setter::{
    ValueSetterChangeLogDiscriminant, ValueSetterChangeLogEntry, ValueSetterMessageGenerator,
    ValueSetterTag,
};
use crate::interface::traits::CallMessageGenerator;

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
#[derive(Clone, Debug, derive_more::From)]
pub enum BasicChangeLogEntry<S: Spec> {
    /// Changes from the bank module
    Bank(BankChangeLogEntry<S>),
    /// Changes from the value setter module
    ValueSetter(ValueSetterChangeLogEntry),
}

/// The available discriminants for the [`BasicChangeLogEntry`]
#[derive(Debug, Clone, PartialEq, Eq, Derivative, derive_more::From)]
#[derivative(Hash(bound = "S: Spec"))]
pub enum BasicChangeLogDiscriminant<S: Spec> {
    /// The types of changes from the bank module
    Bank(BankChangeLogDiscriminant<S>),
    /// The types of changes from the value setter module
    ValueSetter(ValueSetterChangeLogDiscriminant),
}

impl<'a, S: Spec> From<&'a BasicChangeLogEntry<S>> for BasicChangeLogDiscriminant<S> {
    fn from(value: &'a BasicChangeLogEntry<S>) -> Self {
        match value {
            BasicChangeLogEntry::Bank(entry) => BasicChangeLogDiscriminant::Bank(entry.into()),
            BasicChangeLogEntry::ValueSetter(entry) => {
                BasicChangeLogDiscriminant::ValueSetter(entry.into())
            }
        }
    }
}

/// Asserts all the [`BasicChangeLogEntry`] against the existing state
pub async fn assert_logs_against_state<S: Spec>(
    logs: Vec<BasicChangeLogEntry<S>>,
    bank_generator: &BankMessageGenerator<S>,
    value_setter_generator: &ValueSetterMessageGenerator<S>,
    config: &BasicClientConfig,
) -> anyhow::Result<()> {
    let mut seen_entries: HashSet<BasicChangeLogDiscriminant<S>> = HashSet::new();

    for log in logs.iter().rev() {
        if seen_entries.contains(&log.into()) {
            continue;
        }

        seen_entries.insert(log.into());

        match log {
            BasicChangeLogEntry::Bank(bank_changelog_entry) => {
                bank_generator
                    .assert_state(config.clone().into(), bank_changelog_entry)
                    .await?;
            }
            BasicChangeLogEntry::ValueSetter(value_setter_changelog_entry) => {
                value_setter_generator
                    .assert_state(config.clone().into(), value_setter_changelog_entry)
                    .await?;
            }
        }
    }

    Ok(())
}

/// The basic configuration for any rollup http client.
#[derive(Debug, Clone)]
pub struct BasicClientConfig {
    /// The url to query.
    pub url: String,
    /// The rollup height to query, if necessary.
    pub rollup_height: Option<u64>,
}
