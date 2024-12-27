use std::sync::Arc;

use derivative::Derivative;
use serde::{Deserialize, Serialize};
use sov_modules_api::Spec;

use super::bank::harness_interface::BankHarness;
use super::bank::{BankChangeLogEntry, BankMessageGenerator, BankTag};
use super::factory::CallMessageFactory;
use super::value_setter::{
    ValueSetterChangeLogEntry, ValueSetterHarness, ValueSetterMessageGenerator, ValueSetterTag,
};
use crate::interface::traits::CallMessageGenerator;
use crate::HarnessModule;

/// A basic call message generator factory that can be used with modules internal to the sovereign sdk
pub type BasicCallMessageFactory<S, RT, Acct = ()> =
    CallMessageFactory<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig, Acct>;
/// A helper type that corresponds to bank modules compatible with the basic harness
pub type BasicBankHarness<S, RT, Acct = ()> =
    BankHarness<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig, Acct>;
/// A helper type that corresponds to value setter modules compatible with the basic harness
pub type BasicValueSetterHarness<S, RT, Acct = ()> =
    ValueSetterHarness<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig, Acct>;
/// A helper type that contains a reference to a basic module
pub type BasicModuleRef<S, RT, Acct = ()> =
    Arc<dyn HarnessModule<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig, Acct>>;

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
#[derive(Clone, Debug, derive_more::From, PartialEq, Deserialize, Serialize)]
pub enum BasicChangeLogEntry<S: Spec> {
    /// Changes from the bank module
    Bank(BankChangeLogEntry<S>),
    /// Changes from the value setter module
    ValueSetter(ValueSetterChangeLogEntry),
}

/// Asserts all the [`BasicChangeLogEntry`] against the existing state
pub async fn assert_logs_against_state<S: Spec>(
    logs: Vec<BasicChangeLogEntry<S>>,
    bank_generator: &BankMessageGenerator<S>,
    value_setter_generator: &ValueSetterMessageGenerator<S>,
    config: &BasicClientConfig,
) -> anyhow::Result<()> {
    let mut seen_entries: Vec<BasicChangeLogEntry<S>> = Vec::new();

    for log in logs.iter().rev() {
        if seen_entries.contains(log) {
            continue;
        }

        seen_entries.push(log.clone());

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
