use anyhow::Result;
use sov_modules_api::prelude::*;
use sov_modules_api::WorkingSet;

use super::VecSetter;

impl<S: sov_modules_api::Spec> VecSetter<S> {
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
