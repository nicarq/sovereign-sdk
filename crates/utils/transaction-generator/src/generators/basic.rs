//! Implements call message generation for the most widely used modules such that
//! the generator can be plugged into any [`Runtime`] implementation.

use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::Arc;

use derivative::Derivative;
use sov_modules_api::prelude::{arbitrary, tokio};
use sov_modules_api::{DispatchCall, EncodeCall, PrivateKey, Spec};
use sov_modules_stf_blueprint::Runtime;

use super::bank::{BankChangeLogEntry, BankMessageGenerator, Tag as BankTag};
use super::value_setter::{Tag as ValueSetterTag, ValueSetterMessageGenerator};
use crate::generators::value_setter::ValueSetterChangeLogEntry;
use crate::interface::{
    CallMessageGenerator, Distribution, GeneratedMessage, GeneratorState, GeneratorStateMapper,
    MessageValidity,
};
use crate::state::{AccountState, State};

/// Generates call messages for the most widely used modules generically.
///
/// Each instance has its own state, which is some subset of the world state. Callers
/// may instantiate multiple generators and run them in parallel so long as the initial
/// states of the generators are fully disjoin.
pub struct BasicCallMessageGenerator<RT, S: Spec, Acct = ()> {
    /// Controls the relative frequency of messages from each supported module.
    pub module_distribution: Distribution<{ SUPPORTED_MODULES.len() }>,

    bank: BankMessageGenerator<S>,
    value_setter: ValueSetterMessageGenerator<S>,

    // TODO: Add other modules here,
    phantom: PhantomData<(RT, Acct)>,
}

impl<RT: EncodeCall<sov_bank::Bank<S>>, S: Spec, Acct> BasicCallMessageGenerator<RT, S, Acct> {
    /// Instantiate a new [`BasicCallMessageGenerator`] with the given
    /// subset of state.
    pub fn new(
        module_distribution: Distribution<{ SUPPORTED_MODULES.len() }>,
        bank_generator: BankMessageGenerator<S>,
        value_setter_generator: ValueSetterMessageGenerator<S>,
    ) -> Self {
        Self {
            module_distribution,
            bank: bank_generator,
            value_setter: value_setter_generator,
            phantom: PhantomData,
        }
    }

    /// Add a value setter admin account to the generator state
    pub fn add_value_setter_admin(
        &self,
        value_setter_admin: AccountState<S, Acct>,
        generator_state: &mut impl GeneratorState<S, AccountView = AccountState<S, Acct>, Tag = Tag>,
    ) where
        Acct: Clone,
    {
        generator_state.update_account(
            &S::Address::from(&value_setter_admin.private_key.pub_key()),
            value_setter_admin,
        );
    }
}

/// The set of tags supported by the [`BasicCallMessageGenerator`].
// TODO: Macro generate all of this
#[derive(Clone, Copy, Derivative, Debug, derive_more::From)]
#[derivative(PartialEq, Eq, Hash)]
pub enum Tag {
    /// Tags for the bank module
    Bank(BankTag),
    /// Tags for the value setter module
    ValueSetter(ValueSetterTag),
}

/// The set of change log entries supported by the [`BasicCallMessageGenerator`].
#[derive(Clone, Debug, strum::EnumDiscriminants, derive_more::From)]
#[strum_discriminants(name(SupportedModules))]
pub enum BasicChangelogEntry<S: Spec> {
    /// Changes from the bank module
    Bank(BankChangeLogEntry<S>),
    /// Changes from the value setter module
    ValueSetter(ValueSetterChangeLogEntry),
}

/// The list of modules supported by the [`BasicCallMessageGenerator`].
pub const SUPPORTED_MODULES: &[SupportedModules] =
    &[SupportedModules::Bank, SupportedModules::ValueSetter];

/// The basic configuration for any rollup http client.
#[derive(Debug, Clone)]
pub struct BasicClientConfig {
    /// The url to query.
    pub url: String,
    /// The rollup height to query, if necessary.
    pub rollup_height: Option<u64>,
}

impl<RT: Runtime<S>, S: Spec, BonusAcctData: Debug + Clone + Default + 'static>
    BasicCallMessageGenerator<RT, S, BonusAcctData>
where
    RT: EncodeCall<sov_bank::Bank<S>> + EncodeCall<sov_value_setter::ValueSetter<S>>,
{
    /// Generate call messages needed to properly setup the generator.
    #[allow(clippy::type_complexity)]
    pub fn generate_setup_messages<
        Tag: Clone + Eq + Hash + Debug + From<BankTag> + From<ValueSetterTag>,
    >(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut State<S, Tag, BonusAcctData>,
    ) -> arbitrary::Result<
        Vec<GeneratedMessage<S, <RT as DispatchCall>::Decodable, BasicChangelogEntry<S>>>,
    > {
        let bank = self
            .bank
            .generate_setup_messages(
                u,
                &mut GeneratorStateMapper::<_, _, Tag, _>::new(generator_state),
            )?
            .into_iter()
            .map(|m| GeneratedMessage {
                message: <RT as EncodeCall<sov_bank::Bank<S>>>::to_decodable(m.message),
                sender: m.sender,
                changes: m.changes.into_iter().map(Into::into).collect(),
            });

        let value_setter = self
            .value_setter
            .generate_setup_messages(
                u,
                &mut GeneratorStateMapper::<_, _, Tag, _>::new(generator_state),
            )?
            .into_iter()
            .map(|m| GeneratedMessage {
                message: <RT as EncodeCall<sov_value_setter::ValueSetter<S>>>::to_decodable(
                    m.message,
                ),
                sender: m.sender,
                changes: m.changes.into_iter().map(Into::into).collect(),
            });

        Ok(bank.chain(value_setter).collect())
    }

    /// Generates a call message for the modules supported by this generator.
    pub fn generate_call_message<
        Tag: Clone + Eq + Hash + Debug + From<BankTag> + From<ValueSetterTag>,
    >(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut State<S, Tag, BonusAcctData>,
        validity: MessageValidity,
    ) -> arbitrary::Result<
        GeneratedMessage<S, <RT as DispatchCall>::Decodable, BasicChangelogEntry<S>>,
    > {
        let module = *self.module_distribution.select_from(SUPPORTED_MODULES, u)?;
        let GeneratedMessage::<S, <RT as DispatchCall>::Decodable, BasicChangelogEntry<S>> {
            message,
            sender,
            changes,
        } = match module {
            SupportedModules::Bank => {
                let generated_message = self.bank.generate_call_message(
                    u,
                    &mut GeneratorStateMapper::<_, _, Tag, _>::new(generator_state),
                    validity,
                )?;

                GeneratedMessage {
                    message: <RT as EncodeCall<sov_bank::Bank<S>>>::to_decodable(
                        generated_message.message,
                    ),
                    sender: generated_message.sender,
                    changes: generated_message
                        .changes
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                }
            }
            SupportedModules::ValueSetter => {
                let generated_message = self.value_setter.generate_call_message(
                    u,
                    &mut GeneratorStateMapper::<_, _, Tag, _>::new(generator_state),
                    validity,
                )?;

                GeneratedMessage {
                    message: <RT as EncodeCall<sov_value_setter::ValueSetter<S>>>::to_decodable(
                        generated_message.message,
                    ),
                    sender: generated_message.sender,
                    changes: generated_message
                        .changes
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                }
            }
        };

        Ok(GeneratedMessage {
            message,
            sender,
            changes: changes.into_iter().map(Into::into).collect(),
        })
    }

    /// Assert an incremental change of state of the rollup.
    pub async fn assert_incremental_state(
        &self,
        rollup_state_accessor: BasicClientConfig,
        changes: Vec<BasicChangelogEntry<S>>,
    ) -> Result<(), anyhow::Error> {
        // TODO: Add other module changes here
        let mut bank_changes = vec![];
        let mut value_setter_changes = vec![];
        let mut joinset = tokio::task::JoinSet::new();
        let accessor = Arc::new(rollup_state_accessor.clone());

        for change in changes {
            match change {
                BasicChangelogEntry::Bank(change) => bank_changes.push(change),
                BasicChangelogEntry::ValueSetter(change) => value_setter_changes.push(change),
            }
        }

        let accessor_cloned = accessor.clone();
        let bank_generator = self.bank.clone();

        joinset.spawn(async move {
            bank_generator
                .assert_state(accessor_cloned.into(), bank_changes)
                .await
                .unwrap_or_else(|e| {
                    panic!("Bank module failed to assert incremental state: {}", e)
                });
        });

        let accessor_cloned = accessor.clone();
        let value_setter_generator = self.value_setter.clone();

        joinset.spawn(async move {
            value_setter_generator
                .assert_state(accessor_cloned.into(), value_setter_changes)
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "Value setter module failed to assert incremental state: {}",
                        e
                    )
                });
        });

        while let Some(result) = joinset.join_next().await {
            result?;
        }

        Ok(())
    }
}
