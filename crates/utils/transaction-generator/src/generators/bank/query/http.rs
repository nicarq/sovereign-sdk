use std::marker::PhantomData;

use sov_bank::TokenId;
use sov_modules_api::prelude::axum::async_trait;
use sov_modules_api::Spec;
use sov_node_client::NodeClient;

use super::BankClient;
use crate::generators::basic::BasicClientConfig;

/// An http client for querying the state needed by the bank generator
pub struct HttpBankClient<S: Spec> {
    client: NodeClient,
    phantom: PhantomData<S>,
    rollup_height: Option<u64>,
}

impl<S: Spec> From<BasicClientConfig> for HttpBankClient<S> {
    fn from(config: BasicClientConfig) -> Self {
        Self {
            rollup_height: config.rollup_height,
            phantom: Default::default(),
            client: NodeClient::new_unchecked(&config.url),
        }
    }
}

#[async_trait]
impl<S: Spec> BankClient<S> for HttpBankClient<S> {
    async fn get_balance(
        &self,
        user: &<S as sov_modules_api::Spec>::Address,
        token_id: TokenId,
    ) -> sov_bank::Amount {
        self.client
            .get_balance::<S>(user, &token_id, self.rollup_height)
            .await
            .unwrap()
    }
}
