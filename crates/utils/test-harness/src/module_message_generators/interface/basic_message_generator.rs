use std::fmt::Debug;
use std::marker::PhantomData;

use derivative::Derivative;
use sov_modules_api::prelude::arbitrary;
use sov_modules_api::{DispatchCall, EncodeCall, Spec};
use sov_modules_stf_blueprint::Runtime;

use super::{CallMessageGenerator, Distribution, GeneratedMessage};
use crate::bank::message_generator::{
    BankAccount, BankChangeLogEntry, BankMessageGenerator, Tag as BankTag,
};
use crate::interface::GeneratorStateMapper;
use crate::transaction_generator::{AccountState, AccountStateView};

pub struct BasicCallMessageGenerator<RT, S: Spec, Acct = ()> {
    config: BasicCallMessageGeneratorConfig<S>,
    bank: BankMessageGenerator<S>,
    // TODO: Add other modules here,
    phantom: PhantomData<(RT, Acct)>,
}

impl<RT, S: Spec, Acct> BasicCallMessageGenerator<RT, S, Acct> {
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

// TODO: Macro generate all of this
#[derive(Clone, Copy, Derivative, Debug)]
#[derivative(PartialEq, Eq, Hash)]
#[derivative(PartialEq(bound = "S: Spec"))]
#[derivative(Eq(bound = "S: Spec"))]
#[derivative(Hash(bound = "S: Spec"))]
pub enum Tag<S: Spec> {
    Bank(<BankMessageGenerator<S> as CallMessageGenerator<S>>::Tag),
}

impl<S: Spec> From<BankTag> for Tag<S> {
    fn from(value: BankTag) -> Self {
        Self::Bank(value)
    }
}

#[derive(Clone, Debug, strum::EnumDiscriminants)]
#[strum_discriminants(name(SupportedModules))]
pub enum BasicChangelogEntry<S: Spec> {
    Bank(<BankMessageGenerator<S> as CallMessageGenerator<S>>::ChangelogEntry),
}

impl<S: Spec> From<BankChangeLogEntry<S>> for BasicChangelogEntry<S> {
    fn from(value: BankChangeLogEntry<S>) -> Self {
        Self::Bank(value)
    }
}

pub const SUPPORTED_MODULES: &[SupportedModules] = &[SupportedModules::Bank];

#[derive(Clone, Debug)]
pub struct BasicCallMessageGeneratorConfig<S: Spec>
where
    BankMessageGenerator<S>: CallMessageGenerator<S>,
{
    pub module_distribution: Distribution<{ SUPPORTED_MODULES.len() }>,
    pub bank: <BankMessageGenerator<S> as CallMessageGenerator<S>>::Config,
}

impl<RT: Runtime<S>, S: Spec, BonusAcctData: Debug + Clone> CallMessageGenerator<S>
    for BasicCallMessageGenerator<RT, S, BonusAcctData>
where
    AccountState<S, BonusAcctData>: Clone + std::fmt::Debug,
    RT: EncodeCall<sov_bank::Bank<S>>,
    // TODO: Remove this bound on RollupStateReader = ()
    BankMessageGenerator<S>: CallMessageGenerator<
        S,
        RollupStateReader = (),
        CallMessage = sov_bank::CallMessage<S>,
        AccountView = BankAccount<S>,
        Tag = BankTag,
        ChangelogEntry: Into<BasicChangelogEntry<S>>,
    >,
{
    type CallMessage = <RT as DispatchCall>::Decodable;

    type Tag = Tag<S>;

    type AccountView = AccountStateView<S, BonusAcctData>;

    type RollupStateReader = ();

    type ChangelogEntry = BasicChangelogEntry<S>;

    type Config = BasicCallMessageGeneratorConfig<S>;

    fn set_config(&mut self, config: Self::Config) {
        self.bank.set_config(config.bank);
    }

    // We need to apply Bank state to G::State if AccountState; Apply<To> G::State
    fn generate_call_message(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        rollup_state_accessor: &Self::RollupStateReader,
        generator_state: &mut impl super::GeneratorState<
            S,
            AccountView = AccountStateView<S, BonusAcctData>,
            Tag = Self::Tag,
        >,
        validity: super::MessageValidity,
    ) -> arbitrary::Result<super::GeneratedMessage<S, Self::CallMessage, Self::ChangelogEntry>>
    {
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
                rollup_state_accessor,
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
        _generator_state: &mut impl super::GeneratorState<S, AccountView = Self::AccountView>,
    ) -> Result<(), anyhow::Error> {
        todo!()
    }

    fn assert_incremental_state(
        &self,
        _rollup_state_accessor: &Self::RollupStateReader,
        _changes: Vec<Self::ChangelogEntry>,
    ) -> Result<(), anyhow::Error> {
        todo!()
    }
}
