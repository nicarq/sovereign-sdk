//! Implements call message generation for the most widely used modules such that
//! the generator can be plugged into any [`Runtime`] implementation.

use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::Arc;

use derivative::Derivative;
use sov_modules_api::prelude::axum::async_trait;
use sov_modules_api::prelude::{arbitrary, tokio};
use sov_modules_api::{DispatchCall, EncodeCall, PrivateKey, Spec};
use sov_modules_stf_blueprint::Runtime;

use super::bank::{BankAccount, BankChangeLogEntry, BankMessageGenerator, Tag as BankTag};
use super::value_setter::{Tag as ValueSetterTag, ValueSetterMessageGenerator};
use crate::generators::value_setter::{ValueSetterAccount, ValueSetterChangeLogEntry};
use crate::interface::{
    CallMessageGenerator, Distribution, GeneratedMessage, GeneratorState, GeneratorStateMapper,
    MessageValidity, Percent,
};
use crate::state::{AccountState, AccountStateView};

/// Generates call messages for the most widely used modules generically.
///
/// Each instance has its own state, which is some subset of the world state. Callers
/// may instantiate multiple generators and run them in parallel so long as the initial
/// states of the generators are fully disjoin.
pub struct BasicCallMessageGenerator<RT, S: Spec, Acct = ()> {
    config: BasicCallMessageGeneratorConfig<S>,
    bank: BankMessageGenerator<S>,
    value_setter: ValueSetterMessageGenerator<S>,

    // TODO: Add other modules here,
    phantom: PhantomData<(RT, Acct)>,
}

impl<RT: EncodeCall<sov_bank::Bank<S>>, S: Spec, Acct> BasicCallMessageGenerator<RT, S, Acct> {
    /// Instantiate a new [`BasicCallMessageGenerator`] with the given
    /// subset of state.
    pub fn new(
        config: BasicCallMessageGeneratorConfig<S>,
        bank_generator: BankMessageGenerator<S>,
        value_setter_generator: ValueSetterMessageGenerator<S>,
    ) -> Self {
        Self {
            config,
            bank: bank_generator,
            value_setter: value_setter_generator,
            phantom: PhantomData,
        }
    }

    /// Generate an initial `CreateToken` message to get the generator into a usable state.
    pub fn generate_initial_token(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut impl GeneratorState<
            S,
            AccountView = AccountStateView<S, Tag<S>, Acct>,
            Tag = Tag<S>,
        >,
    ) -> GeneratedMessage<S, <RT as DispatchCall>::Decodable, BasicChangelogEntry<S>> {
        self.bank.address_creation_rate = Percent::one_hundred();
        let GeneratedMessage {
            message,
            sender,
            changes,
        } = self
            .bank
            .generate_create_token(
                u,
                &mut GeneratorStateMapper::<_, _, BankTag>::new(generator_state),
                MessageValidity::Valid,
            )
            .expect("Valid token creation can't fail");
        self.bank.address_creation_rate = self.config.bank.address_creation_rate;
        GeneratedMessage {
            message: <RT as EncodeCall<sov_bank::Bank<S>>>::to_decodable(message),
            sender,
            changes: changes.into_iter().map(Into::into).collect(),
        }
    }

    /// Add a value setter admin account to the generator state
    pub fn add_value_setter_admin(
        &self,
        value_setter_admin: AccountState<S, Acct>,
        generator_state: &mut impl GeneratorState<
            S,
            AccountView = AccountStateView<S, Tag<S>, Acct>,
            Tag = Tag<S>,
        >,
    ) where
        Acct: Clone,
    {
        generator_state.update_account(
            S::Address::from(&value_setter_admin.private_key.pub_key()),
            (&value_setter_admin).into(),
        );
    }
}

/// The set of tags supported by the [`BasicCallMessageGenerator`].
// TODO: Macro generate all of this
#[derive(Clone, Copy, Derivative, Debug)]
#[derivative(PartialEq, Eq, Hash)]
#[derivative(PartialEq(bound = "S: Spec"))]
#[derivative(Eq(bound = "S: Spec"))]
#[derivative(Hash(bound = "S: Spec"))]
pub enum Tag<S: Spec> {
    /// Tags for the bank module
    Bank(<BankMessageGenerator<S> as CallMessageGenerator<S>>::Tag),
    /// Tags for the value setter module
    ValueSetter(<ValueSetterMessageGenerator<S> as CallMessageGenerator<S>>::Tag),
}

impl<S: Spec> From<BankTag> for Tag<S> {
    fn from(value: BankTag) -> Self {
        Self::Bank(value)
    }
}

impl<S: Spec> From<ValueSetterTag> for Tag<S> {
    fn from(value: ValueSetterTag) -> Self {
        Self::ValueSetter(value)
    }
}

/// The set of change log entries supported by the [`BasicCallMessageGenerator`].
#[derive(Clone, Debug, strum::EnumDiscriminants)]
#[strum_discriminants(name(SupportedModules))]
pub enum BasicChangelogEntry<S: Spec> {
    /// Changes from the bank module
    Bank(<BankMessageGenerator<S> as CallMessageGenerator<S>>::ChangelogEntry),
    /// Changes from the value setter module
    ValueSetter(<ValueSetterMessageGenerator<S> as CallMessageGenerator<S>>::ChangelogEntry),
}

impl<S: Spec> From<BankChangeLogEntry<S>> for BasicChangelogEntry<S> {
    fn from(value: BankChangeLogEntry<S>) -> Self {
        Self::Bank(value)
    }
}

impl<S: Spec> From<ValueSetterChangeLogEntry> for BasicChangelogEntry<S> {
    fn from(value: ValueSetterChangeLogEntry) -> Self {
        Self::ValueSetter(value)
    }
}

/// The list of modules supported by the [`BasicCallMessageGenerator`].
pub const SUPPORTED_MODULES: &[SupportedModules] =
    &[SupportedModules::Bank, SupportedModules::ValueSetter];

/// Configuratino of a [`BasicCallMessageGenerator`].
#[derive(Clone, Debug)]
pub struct BasicCallMessageGeneratorConfig<S: Spec>
where
    BankMessageGenerator<S>: CallMessageGenerator<S>,
{
    /// Controls the relative frequency of messages from each supported module.
    pub module_distribution: Distribution<{ SUPPORTED_MODULES.len() }>,
    /// Configuration for the bank module.
    pub bank: <BankMessageGenerator<S> as CallMessageGenerator<S>>::Config,
    /// Configuration for the value setter module.
    pub value_setter: <ValueSetterMessageGenerator<S> as CallMessageGenerator<S>>::Config,
}

/// The basic configuration for any rollup http client.
#[derive(Debug, Clone)]
pub struct BasicClientConfig {
    /// The url to query.
    pub url: String,
    /// The rollup height to query, if necessary.
    pub rollup_height: Option<u64>,
}

#[async_trait]
impl<RT: Runtime<S>, S: Spec, BonusAcctData: Debug + Clone> CallMessageGenerator<S>
    for BasicCallMessageGenerator<RT, S, BonusAcctData>
where
    Self: Send + Sync + 'static,
    AccountState<S, BonusAcctData>: Clone + std::fmt::Debug,
    RT: EncodeCall<sov_bank::Bank<S>> + EncodeCall<sov_value_setter::ValueSetter<S>>,
    BankMessageGenerator<S>: CallMessageGenerator<
        S,
        RollupStateReader: From<Arc<BasicClientConfig>>,
        CallMessage = sov_bank::CallMessage<S>,
        AccountView = BankAccount<S>,
        Tag = BankTag,
        ChangelogEntry: Into<BasicChangelogEntry<S>>,
    >,
    ValueSetterMessageGenerator<S>: CallMessageGenerator<
        S,
        RollupStateReader: From<Arc<BasicClientConfig>>,
        CallMessage = sov_value_setter::CallMessage,
        AccountView = ValueSetterAccount<S>,
        Tag = ValueSetterTag,
        ChangelogEntry: Into<BasicChangelogEntry<S>>,
    >,
{
    type CallMessage = <RT as DispatchCall>::Decodable;

    type Tag = Tag<S>;

    type AccountView = AccountStateView<S, Tag<S>, BonusAcctData>;

    type RollupStateReader = BasicClientConfig;

    type ChangelogEntry = BasicChangelogEntry<S>;

    type Config = BasicCallMessageGeneratorConfig<S>;

    fn set_config(&mut self, config: Self::Config) {
        self.bank.set_config(config.bank);
    }

    // We need to apply Bank state to G::State if AccountState; Apply<To> G::State
    fn generate_call_message(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut impl GeneratorState<
            S,
            AccountView = AccountStateView<S, Tag<S>, BonusAcctData>,
            Tag = Self::Tag,
        >,
        validity: MessageValidity,
    ) -> arbitrary::Result<GeneratedMessage<S, <RT as DispatchCall>::Decodable, Self::ChangelogEntry>>
    {
        let module = *self
            .config
            .module_distribution
            .select_from(SUPPORTED_MODULES, u)?;
        let GeneratedMessage::<S, <RT as DispatchCall>::Decodable, _> {
            message,
            sender,
            changes,
        } = match module {
            SupportedModules::Bank => {
                let generated_message = self.bank.generate_call_message(
                    u,
                    &mut GeneratorStateMapper::<_, _, BankTag>::new(generator_state),
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
                    &mut GeneratorStateMapper::<_, _, ValueSetterTag>::new(generator_state),
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

    fn assert_full_state(
        &self,
        _rollup_state_accessor: &Self::RollupStateReader,
        _generator_state: &mut impl GeneratorState<S, AccountView = Self::AccountView>,
    ) -> Result<(), anyhow::Error> {
        todo!()
    }

    async fn assert_incremental_state(
        &self,
        rollup_state_accessor: Self::RollupStateReader,
        changes: Vec<Self::ChangelogEntry>,
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
                .assert_incremental_state(accessor_cloned.into(), bank_changes)
                .await
                .unwrap_or_else(|e| {
                    panic!("Bank module failed to assert incremental state: {}", e)
                });
        });

        let accessor_cloned = accessor.clone();
        let value_setter_generator = self.value_setter.clone();

        joinset.spawn(async move {
            value_setter_generator
                .assert_incremental_state(accessor_cloned.into(), value_setter_changes)
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
