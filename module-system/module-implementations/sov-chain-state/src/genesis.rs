use anyhow::Result;
use serde::{Deserialize, Serialize};
use sov_modules_api::da::Time;
use sov_modules_api::{Gas, KernelWorkingSet, Spec};

use crate::ChainState;

/// Initial configuration of the chain state
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ChainStateConfig<S: Spec> {
    /// The time at genesis
    pub current_time: Time,
    /// The initial gas price for the genesis block
    /// TODO(@theochap) `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/469>`: this field should be replaced with a constant value defined in the `constants{.test}.json` file.
    /// This is not yet the case because that would break the tests that set the initial gas price to zero.
    pub initial_base_fee_per_gas: <S::Gas as Gas>::Price,
}

impl<S: sov_modules_api::Spec, Da: sov_modules_api::DaSpec> ChainState<S, Da> {
    pub(crate) fn init_module(
        &self,
        config: &<Self as sov_modules_api::KernelModule>::Config,
        working_set: &mut KernelWorkingSet<S>,
    ) -> Result<()> {
        self.true_slot_number.set(&0, working_set);
        self.next_visible_slot_number.set(&1, working_set);

        self.time
            .set_true_current(&config.current_time, working_set);

        self.initial_base_fee_per_gas
            .set(&config.initial_base_fee_per_gas, working_set);

        Ok(())
    }
}
