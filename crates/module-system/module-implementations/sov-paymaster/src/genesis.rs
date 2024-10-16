use anyhow::Result;
use serde::{Deserialize, Serialize};
use sov_modules_api::{DaSpec, GenesisState, Module, Spec};

use crate::call::PayeePolicyList;
use crate::{Paymaster, PaymasterPolicy};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PaymasterConfig<S: Spec> {
    pub payers: Vec<PaymasterSetup<S>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S: Spec")]
pub struct PaymasterSetup<S: Spec> {
    pub payer_address: S::Address,
    pub policy: PaymasterPolicy<S, PayeePolicyList<S>>,
    pub sequencers_to_register: Vec<<S::Da as DaSpec>::Address>,
}

impl<S: Spec> Paymaster<S> {
    pub(crate) fn init_module(
        &self,
        config: &<Self as Module>::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<()> {
        for PaymasterSetup {
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
