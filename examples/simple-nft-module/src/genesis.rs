use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sov_modules_api::prelude::*;
use sov_modules_api::{Spec, WorkingSet};

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
        working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        self.admin.set(&config.admin, working_set);
        for (id, owner) in config.owners.iter() {
            if self.owners.get(id, working_set).is_some() {
                bail!("Token id {} already exists", id);
            }
            self.owners.set(id, owner, working_set);
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    type DefaultSpec = sov_modules_api::default_spec::DefaultSpec<sov_mock_zkvm::MockZkVerifier>;
    use sov_modules_api::utils::generate_address;
    use sov_modules_api::Spec;

    use super::NonFungibleTokenConfig;

    #[test]
    fn test_config_serialization() {
        let address: <DefaultSpec as Spec>::Address = generate_address::<DefaultSpec>("admin");
        let owner: <DefaultSpec as Spec>::Address = generate_address::<DefaultSpec>("owner");

        let config = NonFungibleTokenConfig::<DefaultSpec> {
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

        let parsed_config: NonFungibleTokenConfig<DefaultSpec> =
            serde_json::from_str(data).unwrap();
        assert_eq!(config, parsed_config)
    }
}
