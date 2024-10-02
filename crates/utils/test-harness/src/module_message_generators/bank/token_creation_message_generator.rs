use sov_bank::Bank;
use sov_modules_api::Spec;

use crate::module_message_generators::get_prepared_call_message;
use crate::{AccountPool, PreparedCallMessage};

/// The [`TokenCreationMessageGenerator`] structure holds all that is required to prepare
/// [`Bank`] module token-creation call messages, that are sign- and broadcast-able by accounts
/// from the [`AccountPool`].
#[derive(Clone)]
pub struct TokenCreationMessageGenerator<S: Spec> {
    message_count: u64,
    account_pool: AccountPool<S>,
    account_pool_index: u64,
}

impl<S: Spec> TokenCreationMessageGenerator<S> {
    /// Creates a [`TokenCreationMessageGenerator`] with an [`AccountPool`] capable of sending
    /// token-creation module messages.
    pub fn new_from_account_pool(account_pool: AccountPool<S>) -> Self {
        Self {
            message_count: 0,
            account_pool_index: 0,
            account_pool,
        }
    }
}

impl<S: Spec> Iterator for TokenCreationMessageGenerator<S> {
    type Item = PreparedCallMessage<S, Bank<S>>;

    fn next(&mut self) -> Option<Self::Item> {
        let account_pool_index = self.account_pool_index;
        let account = self
            .account_pool
            .get_by_index(&account_pool_index)
            .expect("could not get account from account pool at index: {index}");
        let address = account.address().clone();
        let message_count = self.message_count;

        let prepared_call_message = get_prepared_call_message(
            get_token_creation_call_message(message_count, address),
            account_pool_index,
            None,
        );

        // NOTE So that we iterate through the account pool when sending the messages.
        self.message_count += 1;

        // NOTE: So that we iterate over the account pool indefinitely.
        if self.account_pool.len() as u64 >= self.account_pool_index {
            self.account_pool_index = 0;
        } else {
            self.account_pool_index += 1;
        };

        Some(prepared_call_message)
    }
}

fn get_token_creation_call_message<S: Spec>(
    message_count: u64,
    address: S::Address,
) -> sov_bank::CallMessage<S> {
    sov_bank::CallMessage::CreateToken {
        initial_balance: u64::MAX,
        token_name: format!("token-{message_count}"),
        mint_to_address: address.clone(),
        authorized_minters: vec![address],
    }
}
