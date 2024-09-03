use rand::Rng;
use sov_bank::{Bank, Coins, TokenId};
use sov_modules_api::Spec;

use crate::module_message_generators::get_prepared_call_message;
use crate::{AccountPool, PreparedCallMessage};

/// The [`TokenTransferMessageGenerator`] structure holds all that is required to prepare
/// [`Bank`] module token-creation call messages, that are sign- and broadcast-able by accounts
/// from the [`AccountPool`].
#[derive(Clone)]
pub struct TokenTransferMessageGenerator<S: Spec> {
    message_count: u64,
    account_pool: AccountPool<S>,
    account_pool_index: u64,
    token_id: TokenId,
}

impl<S: Spec> TokenTransferMessageGenerator<S> {
    /// Creates a [`TokenTransferMessageGenerator`] with an [`AccountPool`] capable of signing
    /// token-transfer module messages for the given [`TokenId`].
    pub fn new_from_account_pool(
        account_pool: AccountPool<S>,
        token_id: TokenId,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            message_count: 0,
            account_pool_index: 0,
            account_pool,
            token_id,
        })
    }
}

impl<S: Spec> Iterator for TokenTransferMessageGenerator<S> {
    type Item = PreparedCallMessage<S, Bank<S>>;

    fn next(&mut self) -> Option<Self::Item> {
        let account_pool_index = self.account_pool_index;
        let account = self
            .account_pool
            .get_by_index(&account_pool_index)
            .expect("could not get account from account pool at index: {index}");
        let address = account.address().clone();

        let mut rng = rand::thread_rng();
        // TODO @gskapka have user define the min and max? Also keep ref to rng around for more efficiency.
        let amount = rng.gen_range(1..10_000);

        let prepared_call_message = get_prepared_call_message(
            get_token_transfer_call_message(address, amount, self.token_id)
                .expect("could not get token transfer call message"),
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

fn get_token_transfer_call_message<S: Spec>(
    to: S::Address,
    amount: u64,
    token_id: TokenId,
) -> anyhow::Result<sov_bank::CallMessage<S>> {
    Ok(sov_bank::CallMessage::Transfer {
        to,
        coins: Coins { amount, token_id },
    })
}
