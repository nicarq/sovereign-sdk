use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sov_modules_api::{GenesisState, Spec};

use crate::NonFungibleToken;

/// Config for the NonFungibleToken module.
/// Sets admin and existing owners.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct NonFungibleTokenConfig<S: Spec> {
    /// Admin of the NonFungibleToken module.
    pub admin: S::Address,
    /// Existing owners of the NonFungibleToken module.
    pub owners: Vec<(u64, S::Address)>,
}

impl<S: Spec> NonFungibleToken<S> {
    pub(crate) fn init_module(
        &self,
        config: &<Self as sov_modules_api::Module>::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<()> {
        self.admin.set(&config.admin, state)?;
        for (id, owner) in config.owners.iter() {
            if self.owners.get(id, state)?.is_some() {
                bail!("Token id {} already exists", id);
            }
            self.give_nft(owner, *id, state)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use sov_modules_api::digest::Digest;
    use sov_modules_api::{CryptoSpec, Spec};
    use sov_test_utils::TestSpec;

    use super::NonFungibleTokenConfig;

    /// A utility function for generating an address from a string.
    fn generate_address<S: Spec>(key: &str) -> S::Address
    where
        S::Address: From<[u8; 32]>,
    {
        let hash: [u8; 32] = <S::CryptoSpec as CryptoSpec>::Hasher::digest(key.as_bytes()).into();
        S::Address::from(hash)
    }

    #[test]
    fn test_config_serialization() {
        let address: <TestSpec as Spec>::Address = generate_address::<TestSpec>("admin");
        let owner: <TestSpec as Spec>::Address = generate_address::<TestSpec>("owner");

        let config = NonFungibleTokenConfig::<TestSpec> {
            admin: address,
            owners: vec![(0, owner)],
        };

        let data = r#"
        {
            "admin":"sov1335hded4gyzpt00fpz75mms4m7ck02wgw07yhw9grahj4dzg4yvqk63pml",
            "owners":[
                [0,"sov1fsgzj6t7udv8zhf6zj32mkqhcjcpv52yph5qsdcl0qt94jgdckqsczjm2y"]
            ]
        }"#;

        let parsed_config: NonFungibleTokenConfig<TestSpec> = serde_json::from_str(data).unwrap();
        assert_eq!(config, parsed_config);
    }
}
