use std::marker::PhantomData;

use sov_bank::TokenId;
use sov_modules_api::Spec;
use sov_node_client::NodeClient;

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

impl<S: Spec> HttpBankClient<S> {
    /// Get the balance of a user for a given token
    pub async fn get_balance(
        &self,
        user: &<S as sov_modules_api::Spec>::Address,
        token_id: TokenId,
    ) -> sov_bank::Amount {
        self.client
            .get_balance::<S>(user, &token_id, self.rollup_height)
            .await
            .unwrap()
    }

    /// Get the total supply of a token
    pub async fn get_total_supply(&self, token_id: &TokenId) -> sov_bank::Amount {
        self.client.get_total_supply(token_id).await.unwrap()
    }

    /// Check if a token is frozen
    pub async fn is_frozen(&self, token_id: &TokenId) -> bool {
        self.client
            .get_admins::<S>(token_id)
            .await
            .unwrap()
            .is_empty()
    }
}
