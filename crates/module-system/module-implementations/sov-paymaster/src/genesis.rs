use anyhow::Result;
use serde::{Deserialize, Serialize};
use sov_modules_api::{DaSpec, GenesisState, Module, Spec};

use crate::call::{PayeePolicyList, SafeVec};
use crate::{Paymaster, PaymasterPolicy};

/// The genesis configuration of the paymaster module, consisting of a list of
/// payers and their policies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PaymasterConfig<S: Spec> {
    #[allow(missing_docs)]
    pub payers: SafeVec<PayerGenesisConfig<S>>,
}

/// The genesis config for a particular payer. Unlike standard payer registration,
/// the genesis config for a payer needs to explicitly list which sequencers should
/// use the payer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S: Spec")]
pub struct PayerGenesisConfig<S: Spec> {
    /// The address of the newly registered payer.
    pub payer_address: S::Address,
    /// The policy that this payer will use.
    pub policy: PaymasterPolicy<S, PayeePolicyList<S>>,
    /// The list of sequencers that should be configured to use this payer after genesis. Any sequencers in this
    /// list must also be authorized by the payer's policy.
    pub sequencers_to_register: SafeVec<<S::Da as DaSpec>::Address>,
}

impl<S: Spec> Paymaster<S> {
    pub(crate) fn init_module(
        &self,
        config: &<Self as Module>::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<()> {
        for PayerGenesisConfig {
            payer_address,
            policy,
            sequencers_to_register,
        } in config.payers.iter()
        {
            self.do_registration(
                payer_address,
                sequencers_to_register.iter(),
                policy.clone(),
                state,
            )?;
        }
        Ok(())
    }
}
