//! Implements call message generation for the [`sov_value_setter::ValueSetter`] module.

use std::marker::PhantomData;
use std::sync::Arc;

use http::HttpValueSetterClient;
use sov_modules_api::prelude::arbitrary::Arbitrary;
use sov_modules_api::prelude::axum::async_trait;
use sov_modules_api::prelude::{arbitrary, tokio};
use sov_modules_api::{CryptoSpec, Spec};
use sov_value_setter::{CallMessage, CallMessageDiscriminants};
use strum::VariantArray;

use crate::generators::basic::Tag as BasicTag;
use crate::interface::{
    CallMessageGenerator, Distribution, GeneratedMessage, MessageValidity, Percent, Taggable,
};
use crate::repeatedly;
use crate::state::{AccountState, AccountStateView, ApplyTo};

mod http;

/// The call message discriminants used by the `Bank` module
pub const MESSAGES: &[sov_value_setter::CallMessageDiscriminants] =
    sov_value_setter::CallMessageDiscriminants::VARIANTS;

/// Tags that can be applied to an account
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tag {
    /// The account is an admin
    IsAdmin,
}

/// The state of a value setter account
#[derive(Debug, Clone)]
pub struct ValueSetterAccount<S: Spec> {
    pub(crate) private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
}

impl<S: Spec, Tag: From<BasicTag<S>>, Data> ApplyTo<AccountStateView<S, Tag, Data>>
    for ValueSetterAccount<S>
{
    fn apply_to(self, _account: &mut AccountStateView<S, Tag, Data>) {}
}

impl<S: Spec, T> From<&AccountState<S, T>> for ValueSetterAccount<S> {
    fn from(value: &AccountState<S, T>) -> ValueSetterAccount<S> {
        ValueSetterAccount {
            private_key: value.private_key.clone(),
        }
    }
}

impl<S: Spec, Tag, Data> From<&AccountStateView<S, Tag, Data>> for ValueSetterAccount<S> {
    fn from(value: &AccountStateView<S, Tag, Data>) -> ValueSetterAccount<S> {
        ValueSetterAccount {
            private_key: value
                .private_key
                .as_ref()
                .expect("Cannot construct bank account from empty account view")
                .clone(),
        }
    }
}

impl<S: Spec> Taggable for ValueSetterAccount<S> {
    type Tag = Tag;

    fn add_tag(&mut self, _tag: Self::Tag) {}

    fn remove_tag(&mut self, _tag: Self::Tag) {}

    fn take_tags(&mut self) -> impl IntoIterator<Item = crate::interface::TagAction<Self::Tag>> {
        vec![].into_iter()
    }
}

/// A message generator for the `ValueSetter` module
///
/// ## Note
/// For the [`ValueSetterMessageGenerator`] to be `useful` (ie to be able to send valid messages), users of the testing-harness
/// have to make sure there is an admin account in the [`crate::interface::GeneratorState`] with the tag [`Tag::IsAdmin`].
#[derive(Debug, Clone)]
pub struct ValueSetterMessageGenerator<S: Spec> {
    message_distribution: Distribution<{ MESSAGES.len() }, CallMessageDiscriminants>,
    /// The maximum length of a `SetManyValues` message
    maximum_vec_length: usize,
    phantom: PhantomData<S>,
}

impl<S: Spec> ValueSetterMessageGenerator<S> {
    /// Creates a new [`ValueSetterMessageGenerator`] from a [`ValueSetterGeneratorConfig`]
    pub fn from_config(config: ValueSetterGeneratorConfig<S>) -> Self {
        Self {
            message_distribution: config.message_distribution,
            maximum_vec_length: config.maximum_vec_length,
            phantom: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
/// The configuration of a value setter generator
pub struct ValueSetterGeneratorConfig<S: Spec> {
    /// The distribution of message types
    pub message_distribution: Distribution<{ MESSAGES.len() }, CallMessageDiscriminants>,
    /// The maximum length of a `SetManyValues` message
    pub maximum_vec_length: usize,
    /// The phantom type
    pub phantom: PhantomData<S>,
}

/// A complete description of any possible state change created by the [`ValueSetterMessageGenerator`].
#[derive(Debug, Clone)]
pub enum ValueSetterChangeLogEntry {
    /// The single value was updated
    ValueUpdated {
        /// The new value stored in state
        new_value: u32,
    },
    /// The vector of values was updated
    ManyValuesUpdated {
        /// The new value vector stored in state
        new_values: Vec<u8>,
    },
}

#[async_trait]
impl<S: Spec> CallMessageGenerator<S> for ValueSetterMessageGenerator<S> {
    type CallMessage = sov_value_setter::CallMessage;

    type AccountView = ValueSetterAccount<S>;

    type ChangelogEntry = ValueSetterChangeLogEntry;

    type Tag = Tag;

    type Config = ValueSetterGeneratorConfig<S>;

    type RollupStateReader = HttpValueSetterClient<S>;

    fn set_config(&mut self, config: Self::Config) {
        self.message_distribution = config.message_distribution;
        self.maximum_vec_length = config.maximum_vec_length;
    }

    fn generate_call_message(
        &self,
        u: &mut sov_modules_api::prelude::arbitrary::Unstructured<'_>,
        generator_state: &mut impl crate::interface::GeneratorState<
            S,
            AccountView = Self::AccountView,
            Tag = Self::Tag,
        >,
        validity: crate::interface::MessageValidity,
    ) -> sov_modules_api::prelude::arbitrary::Result<
        crate::interface::GeneratedMessage<S, Self::CallMessage, Self::ChangelogEntry>,
    > {
        match validity {
            MessageValidity::Valid => {
                match self.generate_valid_call_message(u, generator_state) {
                    Ok(m) => Ok(m),
                    Err(InternalMessageGenError::Arbitrary(e)) => Err(e),
                    Err(InternalMessageGenError::AdminNotFound) => {
                        // Generate an invalid message because there is no admin
                        self.generate_call_message(u, generator_state, MessageValidity::Invalid)
                    }
                }
            }
            MessageValidity::Invalid => {
                match self.generate_invalid_call_message(u, generator_state) {
                    Ok(m) => Ok(m),
                    Err(InternalMessageGenError::Arbitrary(e)) => Err(e),
                    Err(InternalMessageGenError::AdminNotFound) => {
                        unreachable!("This should be unreachable, since generating *invalid* value setter calls should be always possible regardless of whether there is an admin")
                    }
                }
            }
        }
    }

    fn assert_full_state(
        &self,
        _rollup_state_accessor: &Self::RollupStateReader,
        _generator_state: &mut impl crate::interface::GeneratorState<
            S,
            AccountView = Self::AccountView,
            Tag = Self::Tag,
        >,
    ) -> Result<(), anyhow::Error> {
        todo!()
    }

    async fn assert_incremental_state(
        &self,
        rollup_state_accessor: Self::RollupStateReader,
        changes: Vec<Self::ChangelogEntry>,
    ) -> Result<(), anyhow::Error> {
        let mut seen_single_value_change = false;
        let mut seen_many_value_change = false;
        let mut joinset = tokio::task::JoinSet::new();
        let accessor = Arc::new(rollup_state_accessor);

        for change in changes.into_iter().rev() {
            match change {
                ValueSetterChangeLogEntry::ValueUpdated { new_value } => {
                    if seen_single_value_change {
                        continue;
                    }
                    seen_single_value_change = true;

                    let accessor = accessor.clone();
                    joinset.spawn(async move {
                        let value = accessor.get_value().await;

                        assert_eq!(value, Some(new_value));
                    });
                }
                ValueSetterChangeLogEntry::ManyValuesUpdated { new_values } => {
                    if seen_many_value_change {
                        continue;
                    }
                    seen_many_value_change = true;

                    let accessor = accessor.clone();
                    joinset.spawn(async move {
                        let values_len = accessor.get_many_values_len().await;

                        assert_eq!(values_len, Some(new_values.len() as u64));

                        for i in 0..values_len.unwrap() {
                            let value = accessor.get_many_values_item(i).await;
                            assert_eq!(value, Some(new_values[i as usize]));
                        }
                    });
                }
            }
        }

        while let Some(result) = joinset.join_next().await {
            result?;
        }

        Ok(())
    }
}

/// Errors that can occur when generating a message
pub enum InternalMessageGenError {
    /// Impossible to find an admin account
    AdminNotFound,
    /// An arbitrary error occurred
    Arbitrary(arbitrary::Error),
}

impl From<arbitrary::Error> for InternalMessageGenError {
    fn from(e: arbitrary::Error) -> Self {
        InternalMessageGenError::Arbitrary(e)
    }
}

impl<S: Spec> ValueSetterMessageGenerator<S> {
    /// We have two types of valid messages:
    /// 1. A message that sets a value and that is sent by an admin
    /// 2. A message that sets multiple values and that is sent by an admin
    pub fn generate_valid_call_message(
        &self,
        u: &mut sov_modules_api::prelude::arbitrary::Unstructured<'_>,
        generator_state: &mut impl crate::interface::GeneratorState<
            S,
            AccountView = ValueSetterAccount<S>,
            Tag = Tag,
        >,
    ) -> Result<GeneratedMessage<S, CallMessage, ValueSetterChangeLogEntry>, InternalMessageGenError>
    {
        let message_type = self.message_distribution.select_value(u)?;
        let Some((_, admin_account)) =
            generator_state.get_random_existing_account_with_tag(Tag::IsAdmin, u)?
        else {
            return Err(InternalMessageGenError::AdminNotFound);
        };

        match message_type {
            CallMessageDiscriminants::SetValue => {
                let value = u32::arbitrary(u)?;

                Ok(GeneratedMessage::new(
                    CallMessage::SetValue(value),
                    admin_account.private_key.clone(),
                    vec![ValueSetterChangeLogEntry::ValueUpdated { new_value: value }],
                ))
            }
            CallMessageDiscriminants::SetManyValues => {
                let length = u.int_in_range(0..=self.maximum_vec_length)?;
                let mut values = Vec::with_capacity(length);

                for _ in 0..length {
                    values.push(u8::arbitrary(u)?);
                }

                Ok(GeneratedMessage::new(
                    CallMessage::SetManyValues(values.clone()),
                    admin_account.private_key.clone(),
                    vec![ValueSetterChangeLogEntry::ManyValuesUpdated { new_values: values }],
                ))
            }
        }
    }

    /// We have two types of invalid messages:
    /// 1. A message that sets a value and that is sent by a non-admin (or if the admin is not set)
    /// 2. A message that sets multiple values and that is sent by a non-admin (or if the admin is not set)
    ///
    /// This method should never fail.
    pub fn generate_invalid_call_message(
        &self,
        u: &mut sov_modules_api::prelude::arbitrary::Unstructured<'_>,
        generator_state: &mut impl crate::interface::GeneratorState<
            S,
            AccountView = ValueSetterAccount<S>,
            Tag = Tag,
        >,
    ) -> Result<GeneratedMessage<S, CallMessage, ValueSetterChangeLogEntry>, InternalMessageGenError>
    {
        let message_type = self.message_distribution.select_value(u)?;

        repeatedly!(
            let (_address, account) = generator_state.get_or_generate(Percent::fifty(), u)?;
            until: !generator_state.has_tag(&_address, Tag::IsAdmin),
            on_failure: panic!("Impossible to get a non-admin account, when there should only be one admin!")
        );

        match message_type {
            CallMessageDiscriminants::SetValue => {
                let value = u32::arbitrary(u)?;
                let message = CallMessage::SetValue(value);
                Ok(GeneratedMessage {
                    message,
                    sender: account.private_key,
                    changes: vec![],
                })
            }
            CallMessageDiscriminants::SetManyValues => {
                let length = u.int_in_range(0..=self.maximum_vec_length)?;
                let mut values = Vec::with_capacity(length);

                for _ in 0..length {
                    values.push(u8::arbitrary(u)?);
                }

                let message = CallMessage::SetManyValues(values);
                Ok(GeneratedMessage {
                    message,
                    sender: account.private_key,
                    changes: vec![],
                })
            }
        }
    }
}
