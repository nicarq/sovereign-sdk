use anyhow::Result;
use serde::{Deserialize, Serialize};
use sov_modules_api::da::{BlockHeaderTrait, Time};
use sov_modules_api::{DaSpec, Gas, GasSpec, GenesisState, Module, OperatingMode, Spec, Zkvm};

use crate::{BlockGasInfo, ChainState, SlotInformation, TransitionHeight};

/// Initial configuration of the chain state
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ChainStateConfig<S: Spec> {
    /// The time at `genesis_da_height` slot according to the DA layer.
    /// So the format depends on DA layer time representation.
    /// Most probably is used for bridging purposes.
    pub current_time: Time,

    /// The mode that the rollup will be operating in.
    pub operating_mode: OperatingMode,

    /// The code commitment to be used for verifying the rollup's execution.
    pub inner_code_commitment: <S::InnerZkvm as Zkvm>::CodeCommitment,

    /// The code commitment to be used for verifying the rollup's execution from genesis to the current slot.
    /// This value is used by the `ProverIncentives` module to verify the proofs posted on the DA layer.
    pub outer_code_commitment: <S::OuterZkvm as Zkvm>::CodeCommitment,

    /// The height of the first DA block.
    pub genesis_da_height: TransitionHeight,
}

impl<S: Spec> ChainState<S> {
    pub(crate) fn init_module(
        &self,
        genesis_slot_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        validity_condition: &<<S as Spec>::Da as DaSpec>::ValidityCondition,
        config: &<Self as Module>::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<()> {
        tracing::info!(
            current_time = ?config.current_time,
            operating_mode = ?config.operating_mode,
            genesis_da_height = config.genesis_da_height,
            inner_code_commitment = ?config.inner_code_commitment,
            outer_code_commitment = ?config.outer_code_commitment,
            "Starting chain state genesis...",
        );

        self.true_rollup_height.set(&0, state)?;
        self.next_visible_rollup_height.set(&0, state)?;
        self.true_to_visible_rollup_height_history
            .set(&0, &0, state)?;

        self.time.set_true_current(&config.current_time, state);
        self.operating_mode.set(&config.operating_mode, state)?;

        self.inner_code_commitment
            .set(&config.inner_code_commitment, state)?;

        self.outer_code_commitment
            .set(&config.outer_code_commitment, state)?;

        self.genesis_da_height
            .set(&config.genesis_da_height, state)?;

        self.slots.initialize(state);

        self.slots.push(
            &SlotInformation::new(
                genesis_slot_header.hash(),
                *validity_condition,
                BlockGasInfo {
                    gas_used: S::Gas::zero(),
                    base_fee_per_gas: S::initial_base_fee_per_gas(),
                    gas_limit: S::Gas::zero(),
                },
            ),
            state,
        );

        self.state_roots.initialize(state);

        Ok(())
    }
}
