//! Implements call message generation for the most widely used modules such that
//! the generator can be plugged into any [`Runtime`] implementation.

use std::fmt::Debug;
use std::marker::PhantomData;

use derivative::Derivative;
use sov_modules_api::prelude::arbitrary;
use sov_modules_api::prelude::axum::async_trait;
use sov_modules_api::{DispatchCall, EncodeCall, Spec};
use sov_modules_stf_blueprint::Runtime;

use super::bank::{BankAccount, BankChangeLogEntry, BankMessageGenerator, Tag as BankTag};
use crate::interface::{
    CallMessageGenerator, Distribution, GeneratedMessage, GeneratorState, GeneratorStateMapper,
    MessageValidity,
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
    // TODO: Add other modules here,
    phantom: PhantomData<(RT, Acct)>,
}

impl<RT, S: Spec, Acct> BasicCallMessageGenerator<RT, S, Acct> {
    /// Instantiate a new [`BasicCallMessageGenerator`] with the given
    /// subset of state.
    pub fn new(
        config: BasicCallMessageGeneratorConfig<S>,
        bank_generator: BankMessageGenerator<S>,
    ) -> Self {
        Self {
            config,
            bank: bank_generator,
            phantom: PhantomData,
        }
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
}

impl<S: Spec> From<BankTag> for Tag<S> {
    fn from(value: BankTag) -> Self {
        Self::Bank(value)
    }
}

/// The set of change log entries supported by the [`BasicCallMessageGenerator`].
#[derive(Clone, Debug, strum::EnumDiscriminants)]
#[strum_discriminants(name(SupportedModules))]
pub enum BasicChangelogEntry<S: Spec> {
    /// Changes from the bank module
    Bank(<BankMessageGenerator<S> as CallMessageGenerator<S>>::ChangelogEntry),
}

impl<S: Spec> From<BankChangeLogEntry<S>> for BasicChangelogEntry<S> {
    fn from(value: BankChangeLogEntry<S>) -> Self {
        Self::Bank(value)
    }
}

/// The list of modules supported by the [`BasicCallMessageGenerator`].
pub const SUPPORTED_MODULES: &[SupportedModules] = &[SupportedModules::Bank];

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
    RT: EncodeCall<sov_bank::Bank<S>>,
    BankMessageGenerator<S>: CallMessageGenerator<
        S,
        RollupStateReader: From<BasicClientConfig>,
        CallMessage = sov_bank::CallMessage<S>,
        AccountView = BankAccount<S>,
        Tag = BankTag,
        ChangelogEntry: Into<BasicChangelogEntry<S>>,
    >,
{
    type CallMessage = <RT as DispatchCall>::Decodable;

    type Tag = Tag<S>;

    type AccountView = AccountStateView<S, BonusAcctData>;

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
            AccountView = AccountStateView<S, BonusAcctData>,
            Tag = Self::Tag,
        >,
        validity: MessageValidity,
    ) -> arbitrary::Result<GeneratedMessage<S, Self::CallMessage, Self::ChangelogEntry>> {
        let module = *self
            .config
            .module_distribution
            .select_from(SUPPORTED_MODULES, u)?;
        let GeneratedMessage {
            message,
            sender,
            changes,
        } = match module {
            SupportedModules::Bank => self.bank.generate_call_message(
                u,
                &mut GeneratorStateMapper::<_, _, BankTag>::new(generator_state),
                validity,
            )?,
        };

        Ok(GeneratedMessage {
            message: RT::to_decodable(message),
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
        for change in changes {
            match change {
                BasicChangelogEntry::Bank(change) => bank_changes.push(change),
            }
        }
        // TODO: join instead of `await`ing here to allow internal parallelism once there
        // are other modules
        self.bank
            .assert_incremental_state(rollup_state_accessor.into(), bank_changes)
            .await?;

        Ok(())
    }
}
