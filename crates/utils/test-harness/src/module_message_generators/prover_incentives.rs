use sov_modules_api::Spec;
use sov_prover_incentives::ProverIncentives;

use crate::constants::DEFAULT_MAX_FEE;
use crate::{AccountPool, PreparedCallMessage};

/// The [`crate::ProverIncentivesMessageGenerator`] structure holds all that is required to prepare
/// [`sov_prover_incentives::ProverIncentives`] module call messages, that are sign- and broadcast-able by accounts
/// from the [`AccountPool`].
#[derive(Clone)]
pub struct ProverIncentivesMessageGenerator<S: Spec> {
    message_count: u64,
    account_pool: AccountPool<S>,
    account_pool_index: u64,
}

impl<S: Spec> ProverIncentivesMessageGenerator<S> {
    /// Creates a [`ProverIncentivesMessageGenerator`] with an [`AccountPool`] capable of sending
    /// [`sov_prover_incentives::ProverIncentives`] module messages.
    pub fn new_from_account_pool(account_pool: AccountPool<S>) -> Self {
        Self {
            account_pool,
            message_count: 0,
            account_pool_index: 0,
        }
    }
}

impl<S: Spec> Iterator for ProverIncentivesMessageGenerator<S> {
    type Item = PreparedCallMessage<S, ProverIncentives<S>>;

    fn next(&mut self) -> Option<Self::Item> {
        let account_pool_index = self.account_pool_index;

        let prepared_call_message = PreparedCallMessage::<S, ProverIncentives<S>> {
            call_message: sov_prover_incentives::CallMessage::Register(2000 + self.message_count),
            account_pool_index,
            max_fee: DEFAULT_MAX_FEE,
        };

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
