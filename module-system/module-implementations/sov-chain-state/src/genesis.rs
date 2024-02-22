use anyhow::Result;
use serde::{Deserialize, Serialize};
use sov_modules_api::da::Time;
use sov_modules_api::{Gas, KernelWorkingSet, Spec, StateValueAccessor};

use crate::{ChainState, GasPriceState};

/// Initial configuration of the chain state
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ChainStateConfig<S: Spec> {
    /// The time at genesis
    pub current_time: Time,
    /// The depth at which the elastic gas price will extract its average target price from the
    /// blocks.
    pub gas_price_blocks_depth: u64,
    /// The elasticity reflects the degree to which the rate of change in price is responsive to
    /// variations in used gas distances from the average target price.
    pub gas_price_maximum_elasticity: i64,
    /// The initial gas price for the genesis block.
    pub initial_gas_price: <S::Gas as Gas>::Price,
    /// The minimum gas price allowed from the state computation.
    pub minimum_gas_price: <S::Gas as Gas>::Price,
}

impl<S: sov_modules_api::Spec, Da: sov_modules_api::DaSpec> ChainState<S, Da> {
    pub(crate) fn init_module(
        &self,
        config: &<Self as sov_modules_api::KernelModule>::Config,
        working_set: &mut KernelWorkingSet<S>,
    ) -> Result<()> {
        self.true_slot_number.set(&0, working_set);
        self.next_visible_slot_number.set(&1, working_set);

        self.time.set_current(&config.current_time, working_set);

        self.gas_price_state.set(
            &GasPriceState {
                blocks_depth: config.gas_price_blocks_depth,
                maximum_elasticity: config.gas_price_maximum_elasticity,
                price: config.initial_gas_price.clone(),
                minimum_price: config.minimum_gas_price.clone(),
            },
            working_set.inner,
        );

        Ok(())
    }
}
