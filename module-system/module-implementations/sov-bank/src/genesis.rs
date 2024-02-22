use anyhow::{bail, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sov_modules_api::{StateMapAccessor, WorkingSet};

use crate::token::Token;
use crate::Bank;

/// Initial configuration for sov-bank module.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(bound = "S::Address: Serialize + DeserializeOwned")]
pub struct BankConfig<S: sov_modules_api::Spec> {
    /// A list of configurations for the initial tokens.
    pub tokens: Vec<TokenConfig<S>>,
}

/// [`TokenConfig`] specifies a configuration used when generating a token for the bank
/// module.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(bound = "S::Address: Serialize + DeserializeOwned")]
pub struct TokenConfig<S: sov_modules_api::Spec> {
    /// The name of the token.
    pub token_name: String,
    /// A vector of tuples containing the initial addresses and balances (as u64)
    pub address_and_balances: Vec<(S::Address, u64)>,
    /// The addresses that are authorized to mint the token.
    pub authorized_minters: Vec<S::Address>,
    /// A salt used to encrypt the token address.
    pub salt: u64,
}

/// The address of the token deployer. For now, set to [0; 32]
pub(crate) const DEPLOYER: [u8; 32] = [0; 32];

impl<S: sov_modules_api::Spec> Bank<S> {
    /// Init an instance of the bank module from the configuration `config`.
    /// For each token in the `config`, calls the [`Token::create`] function to create
    /// the token. Upon success, updates the token set if the token address doesn't already exist.
    pub(crate) fn init_module(
        &self,
        config: &<Self as sov_modules_api::Module>::Config,
        working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        let parent_prefix = self.tokens.prefix();
        for token_config in config.tokens.iter() {
            let (token_address, token) = Token::<S>::create(
                &token_config.token_name,
                &token_config.address_and_balances,
                &token_config.authorized_minters,
                &S::Address::try_from(&DEPLOYER)?,
                token_config.salt,
                parent_prefix,
                working_set,
            )?;

            if self.tokens.get(&token_address, working_set).is_some() {
                bail!("Token address {} already exists", token_address);
            }

            self.tokens.set(&token_address, &token, working_set);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    type DefaultSpec = sov_modules_api::default_spec::DefaultSpec<sov_mock_zkvm::MockZkVerifier>;
    use sov_modules_api::{AddressBech32, Spec};

    use super::*;

    #[test]
    fn test_config_serialization() {
        let address: <DefaultSpec as Spec>::Address = AddressBech32::from_str(
            "sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94",
        )
        .unwrap()
        .into();

        let config = BankConfig::<DefaultSpec> {
            tokens: vec![TokenConfig {
                token_name: "sov-demo-token".to_owned(),
                address_and_balances: vec![(address, 100000000)],
                authorized_minters: vec![address],
                salt: 0,
            }],
        };

        let data = r#"
        {
            "tokens":[
                {
                    "token_name":"sov-demo-token",
                    "address_and_balances":[["sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94",100000000]],
                    "authorized_minters":["sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94"]
                    ,"salt":0
                }
            ]
        }"#;

        let parsed_config: BankConfig<DefaultSpec> = serde_json::from_str(data).unwrap();

        assert_eq!(config, parsed_config)
    }
}
