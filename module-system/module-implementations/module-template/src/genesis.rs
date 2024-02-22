use anyhow::Result;
use sov_modules_api::WorkingSet;

use crate::ExampleModule;

impl<S: sov_modules_api::Spec> ExampleModule<S> {
    pub(crate) fn init_module(
        &self,
        _config: &<Self as sov_modules_api::Module>::Config,
        _working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        Ok(())
    }
}
