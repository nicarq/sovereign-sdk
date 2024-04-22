use anyhow::{bail, Result};
use sov_modules_api::{CryptoSpec, PublicKey, Spec, WorkingSet};

use crate::Accounts;
/// Initial configuration for sov-accounts module.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    bound = "<S::CryptoSpec as CryptoSpec>::PublicKey: serde::Serialize + serde::de::DeserializeOwned"
)]
pub struct AccountConfig<S: Spec> {
    /// Public keys to initialize the rollup.
    pub pub_keys: Vec<<S::CryptoSpec as CryptoSpec>::PublicKey>,
}

impl<S: Spec> Accounts<S> {
    pub(crate) fn init_module(
        &self,
        config: &<Self as sov_modules_api::Module>::Config,
        working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        for pub_key in config.pub_keys.iter() {
            let pub_key_hash = pub_key.secure_hash::<<S::CryptoSpec as CryptoSpec>::Hasher>();
            if self.accounts.get(&pub_key_hash, working_set).is_some() {
                bail!("Account already exists")
            }

            let default_address = pub_key.to_address::<<S::CryptoSpec as CryptoSpec>::Hasher, _>();
            let _ = self.get_or_create_default(&pub_key_hash, &default_address, working_set);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use sov_test_utils::{TestPublicKey, TestSpec};

    use super::*;

    #[test]
    fn test_config_serialization() {
        let pub_key = &TestPublicKey::from_str(
            "1cd4e2d9d5943e6f3d12589d31feee6bb6c11e7b8cd996a393623e207da72cbf",
        )
        .unwrap();

        let config = AccountConfig::<TestSpec> {
            pub_keys: vec![pub_key.clone()],
        };

        let data = r#"
        {
            "pub_keys":["1cd4e2d9d5943e6f3d12589d31feee6bb6c11e7b8cd996a393623e207da72cbf"]
        }"#;

        let parsed_config: AccountConfig<TestSpec> = serde_json::from_str(data).unwrap();
        assert_eq!(parsed_config, config);
    }
}
