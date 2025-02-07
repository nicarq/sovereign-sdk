use anyhow::Result;
use schemars::JsonSchema;
use sov_modules_api::{GenesisState, Spec};

use super::TestModule;

/// Initial configuration for sov-test-module module.
#[derive(Clone, serde::Serialize, serde::Deserialize, Debug, PartialEq, JsonSchema)]
#[schemars(bound = "S: Spec", rename = "TestModuleConfig")]
pub struct TestModuleConfig<S: Spec> {
    /// Admin of the module.
    pub admin: S::Address,
}

impl<S: Spec> TestModule<S> {
    /// Initializes module with the `admin` role.
    pub(crate) fn init_module(
        &self,
        admin_config: &<Self as sov_modules_api::Module>::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<()> {
        self.admin.set(&admin_config.admin, state)?;
        self.begin_rollup_block_hook_count.set(&0, state)?;
        self.end_rollup_block_hook_count.set(&0, state)?;
        self.finalize_hook_count.set(&0, state)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sov_modules_api::prelude::serde_json;
    use sov_modules_api::Spec;
    use sov_test_utils::TestSpec;

    use crate::TestModuleConfig;

    #[test]
    fn test_config_serialization() {
        let admin = <TestSpec as Spec>::Address::from([1; 28]);
        let config = TestModuleConfig::<TestSpec> { admin };

        let data = r#"
        {
            "admin":"sov1qyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszc740j2"
        }"#;

        let parsed_config: TestModuleConfig<TestSpec> = serde_json::from_str(data).unwrap();
        assert_eq!(parsed_config, config);
    }
}
