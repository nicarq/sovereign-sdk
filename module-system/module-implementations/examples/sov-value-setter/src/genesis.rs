use anyhow::Result;
use sov_modules_api::prelude::*;
use sov_modules_api::WorkingSet;

use super::ValueSetter;

/// Initial configuration for sov-value-setter module.
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
pub struct ValueSetterConfig<S: sov_modules_api::Spec> {
    /// Admin of the module.
    pub admin: S::Address,
}

impl<S: sov_modules_api::Spec> ValueSetter<S> {
    /// Initializes module with the `admin` role.
    pub(crate) fn init_module(
        &self,
        admin_config: &<Self as sov_modules_api::Module>::Config,
        working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        self.admin.set(&admin_config.admin, working_set);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    type DefaultSpec = sov_modules_api::default_spec::DefaultSpec<sov_mock_zkvm::MockZkVerifier>;
    use sov_modules_api::Address;

    use crate::ValueSetterConfig;

    #[test]
    fn test_config_serialization() {
        let admin = Address::from([1; 32]);
        let config = ValueSetterConfig::<DefaultSpec> { admin };

        let data = r#"
        {
            "admin":"sov1qyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqs259tk3"
        }"#;

        let parsed_config: ValueSetterConfig<DefaultSpec> = serde_json::from_str(data).unwrap();
        assert_eq!(parsed_config, config);
    }
}
