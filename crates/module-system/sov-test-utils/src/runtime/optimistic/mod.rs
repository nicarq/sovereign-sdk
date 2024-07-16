use std::marker::PhantomData;

pub use genesis::HighLevelOptimisticGenesisConfig;
use sov_modules_api::{DaSpec, Spec};

use crate::runtime::{
    AttesterIncentivesConfig, BankConfig, SequencerConfig, ValueSetter, ValueSetterConfig,
};
use crate::{
    TEST_DEFAULT_USER_STAKE, TEST_LIGHT_CLIENT_FINALIZED_HEIGHT, TEST_MAX_ATTESTED_HEIGHT,
    TEST_ROLLUP_FINALITY_PERIOD,
};

pub mod genesis;
#[cfg(test)]
mod tests;

/// Generates a runtime containing the [`Bank`](sov_bank::Bank), [`AttesterIncentives`](sov_attester_incentives::AttesterIncentives),
/// and [`SequencerRegistry`](sov_sequencer_registry::SequencerRegistry) modules in addition to any provided as arguments`
#[macro_export]
macro_rules! generate_optimistic_runtime {
    ($id:ident <= $($module_name:ident : $module_ty:path),*) => {
        #[derive(
            Default,
            Clone,
            ::sov_modules_api::Genesis,
            ::sov_modules_api::DispatchCall,
            ::sov_modules_api::Event,
            ::sov_modules_api::MessageCodec
        )]
        #[serialization(
            ::borsh::BorshDeserialize,
            ::borsh::BorshSerialize,
            ::serde::Serialize,
            ::serde::Deserialize
        )]
        pub struct __GeneratedRuntimeInternals<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> {
            pub sequencer_registry: $crate::runtime::SequencerRegistry<S, Da>,
            pub attester_incentives: $crate::runtime::AttesterIncentives<S, Da>,
            pub bank: $crate::runtime::Bank<S>,
            $(pub $module_name: $module_ty),*
        }

        pub type $id<S, Da> = $crate::runtime::wrapper::TestRuntimeWrapper<S, Da, __GeneratedRuntimeInternals<S, Da>>;


        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> $crate::runtime::traits::MinimalRuntime<S, Da> for __GeneratedRuntimeInternals<S, Da> {
            fn bank(&self) -> &$crate::runtime::Bank<S> {
                &self.bank
            }

            fn sequencer_registry(&self) -> &$crate::runtime::SequencerRegistry<S, Da> {
                &self.sequencer_registry
            }

            fn base_fee_recipient(&self) -> impl $crate::runtime::Payable<S> {
                ::sov_bank::IntoPayable::to_payable(&self.attester_incentives.id)
            }
        }

        impl <S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> ::sov_modules_api::hooks::TxHooks for __GeneratedRuntimeInternals<S, Da> {
            type Spec = S;
            type TxState = ::sov_modules_api::WorkingSet<S>;

            fn pre_dispatch_tx_hook(
                &self,
                _tx: &::sov_modules_api::transaction::AuthenticatedTransactionData<S>,
                _state: &mut Self::TxState,
            ) -> ::anyhow::Result<()> {
                Ok(())
            }

            fn post_dispatch_tx_hook(
                &self,
                _tx: &::sov_modules_api::transaction::AuthenticatedTransactionData<S>,
                _ctx: &::sov_modules_api::Context<S>,
                _state: &mut Self::TxState,
            ) -> ::anyhow::Result<()> {
                Ok(())
            }
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> $crate::runtime::traits::MinimalGenesis<S> for __GeneratedRuntimeInternals<S, Da> {
            type Da = Da;
            fn sequencer_registry_config(config: &GenesisConfig<S, Da>) -> &<$crate::runtime::SequencerRegistry<S, Self::Da> as ::sov_modules_api::Genesis>::Config {
                &config.sequencer_registry
            }

            fn bank_config(config: &GenesisConfig<S, Da>) -> &<$crate::runtime::Bank<S> as ::sov_modules_api::Genesis>::Config {
                &config.bank
            }
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> GenesisConfig<S, Da> {
            #[allow(unused)]
            pub fn from_minimal_config(minimal_config: $crate::runtime::optimistic::genesis::MinimalOptimisticGenesisConfig<S, Da>,
                $($module_name: <$module_ty as ::sov_modules_api::Genesis>::Config),*
            ) -> Self {
                Self {
                    sequencer_registry: minimal_config.sequencer_registry,
                    attester_incentives: minimal_config.attester_incentives,
                    bank: minimal_config.bank,
                    $(
                        $module_name,
                    )*
                }
            }
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> GenesisConfig<S, Da>
        where <S::InnerZkvm as ::sov_modules_api::Zkvm>::CodeCommitment: Default,
         <S::OuterZkvm as ::sov_modules_api::Zkvm>::CodeCommitment: Default,{
            #[allow(unused)]
            pub fn into_genesis_params(self) -> $crate::runtime::GenesisParams<Self, $crate::runtime::BasicKernelGenesisConfig<S, Da>> {
                $crate::runtime::GenesisParams {
                    runtime: self,
                    kernel: $crate::runtime::BasicKernelGenesisConfig {
                        chain_state: $crate::runtime::ChainStateConfig {
                            current_time: Default::default(),
                            inner_code_commitment: Default::default(),
                            outer_code_commitment: Default::default(),
                            genesis_da_height: 0,
                        }
                    }
                }
            }
        }
    };
}

// TODO: Delete the hookless TestRuntime after upgrading tests to the HookedRuntime
// <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/682>
generate_optimistic_runtime!(TestRuntime <= value_setter: ValueSetter<S>);

/// Admin: single address that will be used as admin and minter.
/// Sequencer is another address that will be used as sequencer.
#[allow(clippy::too_many_arguments)]
pub fn create_genesis_config<S: Spec, Da: DaSpec>(
    admin: S::Address,
    additional_accounts: &[(S::Address, u64)],
    seq_rollup_address: S::Address,
    seq_da_address: Da::Address,
    seq_stake_amount: u64,
    token_name: String,
    init_balance: u64,
) -> GenesisConfig<S, Da> {
    assert!(
        init_balance >= seq_stake_amount,
        "sequencer cannot stake more than its initial balance"
    );
    GenesisConfig {
        value_setter: ValueSetterConfig {
            admin: admin.clone(),
        },
        sequencer_registry: SequencerConfig {
            seq_rollup_address: seq_rollup_address.clone(),
            seq_da_address,
            minimum_bond: seq_stake_amount,
            is_preferred_sequencer: true,
        },
        attester_incentives: AttesterIncentivesConfig {
            minimum_attester_bond: TEST_DEFAULT_USER_STAKE,
            minimum_challenger_bond: TEST_DEFAULT_USER_STAKE,
            initial_attesters: vec![(admin.clone(), TEST_DEFAULT_USER_STAKE)],
            rollup_finality_period: TEST_ROLLUP_FINALITY_PERIOD,
            maximum_attested_height: TEST_MAX_ATTESTED_HEIGHT,
            light_client_finalized_height: TEST_LIGHT_CLIENT_FINALIZED_HEIGHT,
            phantom_data: PhantomData,
        },
        bank: BankConfig {
            gas_token_config: sov_bank::GasTokenConfig {
                token_name: token_name.clone(),
                address_and_balances: {
                    let mut additional_accounts_vec = additional_accounts.to_vec();
                    additional_accounts_vec.append(&mut vec![
                        (seq_rollup_address, init_balance),
                        (admin.clone(), init_balance),
                    ]);
                    additional_accounts_vec
                },
                authorized_minters: vec![admin.clone()],
            },
            tokens: vec![],
        },
    }
}
