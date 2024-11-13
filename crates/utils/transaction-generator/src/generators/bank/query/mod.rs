pub mod http;
use sov_bank::TokenId;
use sov_modules_api::prelude::axum::async_trait;
use sov_modules_api::Spec;

/// A trait which allows access to state values needed by the bank generator
#[async_trait]
pub trait BankClient<S: Spec> {
    /// Gets the user's balance of the given TokenId
    async fn get_balance(&self, user: &S::Address, token: TokenId) -> sov_bank::Amount;
}
