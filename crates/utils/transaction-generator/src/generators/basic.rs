use derivative::Derivative;
use sov_modules_api::Spec;

use super::bank::{BankChangeLogEntry, BankTag};
use super::factory::CallMessageFactory;
use super::value_setter::{ValueSetterChangeLogEntry, ValueSetterTag};

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

/// The basic configuration for any rollup http client.
#[derive(Debug, Clone)]
pub struct BasicClientConfig {
    /// The url to query.
    pub url: String,
    /// The rollup height to query, if necessary.
    pub rollup_height: Option<u64>,
}
