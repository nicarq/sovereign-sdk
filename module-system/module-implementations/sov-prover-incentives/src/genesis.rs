use anyhow::Result;
use serde::{Deserialize, Serialize};
use sov_modules_api::prelude::*;
use sov_modules_api::{DaSpec, Spec, WorkingSet, Zkvm};

use crate::{Amount, BurnRate, ProverIncentives};

/// Configuration of the prover incentives module. Specifies the
/// address of the bonding token, the minimum bond, the commitment to
/// the allowed verifier method and a set of initial provers with their
/// bonding amount.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProverIncentivesConfig<S: sov_modules_api::Spec> {
    /// The address of the account holding the reward token supply
    pub reward_token_supply_address: S::Address,
    /// The burn rate of the reward price for the provers.
    /// The burn rate is a percentage of the base fee that is burned - this prevents provers from proving empty blocks.
    /// This is a constant defined at genesis for now.
    pub reward_burn_rate: BurnRate,
    /// A penalty for provers who submit a proof for transitions that were already proven
    pub proving_penalty: Amount,
    /// The address of the token to be used for bonding.
    pub bonding_token_address: S::Address,
    /// The minimum bond for a prover.
    pub minimum_bond: u64,
    /// A code commitment to be used for verifying proofs
    pub commitment_of_allowed_verifier_method: <<S as Spec>::Zkvm as Zkvm>::CodeCommitment,
    /// A list of initial provers and their bonded amount.
    pub initial_provers: Vec<(S::Address, u64)>,
}

impl<S: sov_modules_api::Spec, Da: DaSpec> ProverIncentives<S, Da> {
    /// Init the [`ProverIncentives`] module using the provided `config`.
    /// Sets the minimum amount necessary to bond, the commitment to the verifier circuit
    /// the bonding token address and builds the set of initial provers.
    pub(crate) fn init_module(
        &self,
        config: &<Self as sov_modules_api::Module>::Config,
        working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        anyhow::ensure!(
            !config.initial_provers.is_empty(),
            "At least one prover must be set at genesis!"
        );

        self.minimum_bond.set(&config.minimum_bond, working_set);
        self.commitment_of_allowed_verifier_method
            .set(&config.commitment_of_allowed_verifier_method, working_set);
        self.bonding_token_address
            .set(&config.bonding_token_address, working_set);
        self.reward_token_supply_address
            .set(&config.reward_token_supply_address, working_set);
        self.reward_burn_rate
            .set(&config.reward_burn_rate, working_set);
        self.proving_penalty
            .set(&config.proving_penalty, working_set);
        self.last_claimed_reward.set(&0, working_set);

        for (prover, bond) in config.initial_provers.iter() {
            self.bond_prover_helper(*bond, prover, working_set)?;
        }

        Ok(())
    }
}
