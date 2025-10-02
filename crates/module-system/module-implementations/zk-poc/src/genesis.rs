use anyhow::Result;
use sov_modules_api::{GenesisState, Module, Spec};

use crate::ZkPoc;

impl<S: Spec> ZkPoc<S> {
    pub(crate) fn init_module(
        &mut self,
        config: &<Self as Module>::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<()> {
        self.method_id.set(&config.method_id, state)?;
        Ok(())
    }
}
