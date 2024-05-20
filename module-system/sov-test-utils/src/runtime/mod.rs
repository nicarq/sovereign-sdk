use std::marker::PhantomData;
use std::path::PathBuf;

use borsh::BorshSerialize;
use sov_attester_incentives::AttesterIncentivesConfig;
pub use sov_bank::{Bank, BankConfig, Coins, TokenConfig, TokenId};
pub use sov_chain_state::ChainStateConfig;
use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockBlob, MockBlock, MockBlockHeader, MockDaSpec};
use sov_modules_api::batch::Batch;
use sov_modules_api::runtime::capabilities::RawTx;
use sov_modules_api::transaction::AuthenticatedTransactionAndRawHash;
use sov_modules_api::{CryptoSpec, DaSpec, Genesis, SlotData, Spec};
use sov_modules_stf_blueprint::{GenesisParams, Runtime, StfBlueprint};
use sov_prover_storage_manager::ProverStorageManager;
use sov_rollup_interface::da::RelevantBlobIters;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
pub use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
use sov_state::{DefaultStorageSpec, ProverStorage};
pub use sov_value_setter::{ValueSetter, ValueSetterConfig};

mod traits;
mod wrapper;
use traits::{MinimalGenesis, PostTxHookRegistry, TestRuntimeHookOverrides};
use wrapper::{TestRuntimeWrapper, WorkingSetClosure};

// Constants used in the genesis configuration of the test runtime
const MIN_USER_BOND: u64 = 10;
const MAX_ATTESTED_HEIGHT: u64 = 0;
const LIGHT_CLIENT_FINALIZED_HEIGHT: u64 = 0;
const ROLLUP_FINALITY_PERIOD: u64 = 1;

/// Generates a runtime containing the [`Bank`], [`AttesterIncentives`](sov_attester_incentives::AttesterIncentives),
/// and [`SequencerRegistry`] modules in addition to any provided as arguments`
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
            pub sequencer_registry: ::sov_sequencer_registry::SequencerRegistry<S, Da>,
            pub attester_incentives: ::sov_attester_incentives::AttesterIncentives<S, Da>,
            pub bank: ::sov_bank::Bank<S>,
            $(pub $module_name: $module_ty),*
        }

        pub type $id<S, Da> = TestRuntimeWrapper<S, Da, __GeneratedRuntimeInternals<S, Da>>;


        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> $crate::runtime::traits::MinimalRuntime<S, Da> for __GeneratedRuntimeInternals<S, Da> {
            fn bank(&self) -> &::sov_bank::Bank<S> {
                &self.bank
            }

            fn sequencer_registry(&self) -> &::sov_sequencer_registry::SequencerRegistry<S, Da> {
                &self.sequencer_registry
            }

            fn attester_incentives(&self) -> &::sov_attester_incentives::AttesterIncentives<S, Da> {
                &self.attester_incentives
            }
        }

        impl <S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> ::sov_modules_api::hooks::TxHooks for __GeneratedRuntimeInternals<S, Da> {
            type Spec = S;
            type TxState = ::sov_modules_api::WorkingSet<S>;

            fn pre_dispatch_tx_hook(
                &self,
                _tx: &::sov_modules_api::transaction::AuthenticatedTransactionData<S>,
                _working_set: &mut Self::TxState,
            ) -> ::anyhow::Result<()> {
                Ok(())
            }

            fn post_dispatch_tx_hook(
                &self,
                _tx: &::sov_modules_api::transaction::AuthenticatedTransactionData<S>,
                _ctx: &::sov_modules_api::Context<S>,
                _working_set: &mut Self::TxState,
            ) -> ::anyhow::Result<()> {
                Ok(())
            }
        }

        impl <S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> TestRuntimeWrapper<S, Da, __GeneratedRuntimeInternals<S, Da>> {
            /// Get a reference to the bank module.
            pub fn bank(&self) -> &::sov_bank::Bank<S> {
                &self.inner.bank
            }

            /// Get a reference to the sequencer registry.
            pub fn sequencer_registry(&self) -> &::sov_sequencer_registry::SequencerRegistry<S, Da> {
                &self.inner.sequencer_registry
            }

            /// Get a reference to the attester incentives module.
            pub fn attester_incentives(&self) -> &::sov_attester_incentives::AttesterIncentives<S, Da> {
                &self.inner.attester_incentives
            }

            $(
            /// Get a references to the $module_name module.
            pub fn $module_name(&self) -> & $module_ty {
                &self.inner.$module_name
            }
            )*
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> $crate::runtime::traits::MinimalGenesis<S> for __GeneratedRuntimeInternals<S, Da> {
            type Da = Da;
            fn sequencer_registry(config: &GenesisConfig<S, Da>) -> &<::sov_sequencer_registry::SequencerRegistry<S, Self::Da> as Genesis>::Config {
                &config.sequencer_registry
            }
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> $crate::runtime::traits::MinimalGenesis<S> for $id<S, Da> {
            type Da = Da;
            fn sequencer_registry(config: &GenesisConfig<S, Da>) -> &<::sov_sequencer_registry::SequencerRegistry<S, Self::Da> as Genesis>::Config {
                &config.sequencer_registry
            }
        }
    };
}

/// Generates a runtime containing the [`Bank`], [`AttesterIncentives`](sov_attester_incentives::AttesterIncentives),
/// and [`SequencerRegistry`] modules in addition to any provided as arguments. The generated runtime has an extensible post
/// transaction hook system that allows for making assertions about the state of the rollup after
/// each transaction. It is meant to be used with the [`run_test`] function.
#[macro_export]
macro_rules! generate_optimistic_runtime_with_test_hooks {
    ($id:ident <= $($module_name:ident : $module_ty:path),*) => {
        generate_optimistic_runtime!( $id <= $($module_name : $module_ty),*);

        impl <S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> $crate::runtime::traits::PostTxHookRegistry<S, Da> for $id <S, Da> {
            fn try_get_next(&self) ->  ::std::option::Option<$crate::runtime::wrapper::WorkingSetClosure<Self>>
            {
                self.hook_action_queue.try_get_next()
            }

            // Add assertions to the post dispatch hook. Callers should provide exactly one assertion per transaction.
            fn add_post_dispatch_tx_hook_actions(&self, closures: Vec<$crate::runtime::wrapper::WorkingSetClosure<Self>>) {
                self.hook_action_queue.insert_all(closures);
            }
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> $crate::runtime::traits::TestRuntimeHookOverrides<S, Da> for $id <S, Da>  {
            // Override the post dispatch hook to run the assertions which
            // were set up using `add_post_dispatch_tx_hook_actions`
            fn post_dispatch_tx_hook_override(
                &self,
                _tx: &::sov_modules_api::transaction::AuthenticatedTransactionData<S>,
                _ctx: &::sov_modules_api::Context<S>,
                working_set: &mut <Self as ::sov_modules_api::hooks::TxHooks>::TxState,
            ) -> ::anyhow::Result<()> {
                let closure = self.try_get_next().expect("Must provide one closure per transaction");
                closure(working_set);
                Ok(())
            }
        }
    }
}

type DefaultSpecWithHasher<S> = DefaultStorageSpec<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>;

/// Run a test on the given runtime
pub fn run_test<RT, S>(
    genesis_config: GenesisParams<<RT as Genesis>::Config, BasicKernelGenesisConfig<S, MockDaSpec>>,
    txs_and_post_checks: Vec<(RawTx, WorkingSetClosure<RT>)>,
    runtime: RT,
) where
    RT: Runtime<S, MockDaSpec>
        + PostTxHookRegistry<S, MockDaSpec>
        + MinimalGenesis<S, Da = MockDaSpec>,
    S: Spec<Storage = ProverStorage<DefaultSpecWithHasher<S>>>,
{
    let stf = StfBlueprint::<S, MockDaSpec, RT, BasicKernel<S, MockDaSpec>>::with_runtime(
        runtime.clone(),
    );
    let (txs, assertions) = txs_and_post_checks.into_iter().unzip();
    runtime.add_post_dispatch_tx_hook_actions(assertions);

    let temp_dir = tempfile::tempdir().unwrap();
    let storage_config = sov_state::config::Config {
        path: PathBuf::from(temp_dir.path()),
    };
    let sequencer_da_address =
        <RT as MinimalGenesis<S>>::sequencer_registry(&genesis_config.runtime).seq_da_address;

    let mut storage_manager = ProverStorageManager::<MockDaSpec, _>::new(storage_config)
        .expect("ProverStorageManager initialization has failed");

    let genesis_block = MockBlock::default();
    let (stf_state, ledger_state) = storage_manager
        .create_state_for(genesis_block.header())
        .unwrap();
    let (state_root, change_set) = stf.init_chain(stf_state, genesis_config);

    storage_manager
        .save_change_set(genesis_block.header(), change_set, ledger_state.into())
        .unwrap();
    // Write it to the database immediately!
    storage_manager.finalize(&genesis_block.header).unwrap();

    let batch = Batch { txs };
    let blob = batch.try_to_vec().unwrap();
    let mut blob = MockBlob::new_with_hash(blob, sequencer_da_address);
    let block_header = MockBlockHeader::from_height(1);

    let (stf_state, _ledger_state) = storage_manager
        .create_state_for(&block_header)
        .expect("Block builds on height zero");
    let relevant_blobs = RelevantBlobIters {
        proof_blobs: vec![],
        batch_blobs: vec![&mut blob],
    };
    stf.apply_slot(
        &state_root,
        stf_state,
        Default::default(),
        &block_header,
        &Default::default(),
        relevant_blobs,
    );
}

// TODO: Delete the hookless TestRuntime after upgrading tests to the HookedRuntime
// <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/682>
generate_optimistic_runtime!(TestRuntime <= value_setter: ValueSetter<S>);
impl<S: Spec, Da: DaSpec> TestRuntimeHookOverrides<S, Da> for TestRuntime<S, Da> {}
pub use framework::{GenesisConfig as HookedRuntimeGenesisConfig, HookedRuntime};
mod framework {
    use super::*;
    generate_optimistic_runtime_with_test_hooks!(HookedRuntime <= value_setter: ValueSetter<S>);
}

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
    validity_condition_checker: Da::Checker,
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
            minimum_attester_bond: MIN_USER_BOND,
            minimum_challenger_bond: MIN_USER_BOND,
            initial_attesters: vec![(admin.clone(), MIN_USER_BOND)],
            rollup_finality_period: ROLLUP_FINALITY_PERIOD,
            maximum_attested_height: MAX_ATTESTED_HEIGHT,
            light_client_finalized_height: LIGHT_CLIENT_FINALIZED_HEIGHT,
            validity_condition_checker,
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

#[cfg(test)]
mod test_rt {

    use sov_kernels::basic::BasicKernelGenesisConfig;
    use sov_mock_da::{MockDaSpec, MockValidityCondChecker};
    use sov_mock_zkvm::MockCodeCommitment;
    use sov_modules_api::transaction::Transaction;
    use sov_modules_api::{Address, EncodeCall, PrivateKey, WorkingSet};
    use sov_modules_stf_blueprint::GenesisParams;

    use super::*;
    use crate::{TestPrivateKey, TestSpec};

    const SEQUENCER_ADDR: [u8; 32] = [42u8; 32];
    generate_optimistic_runtime_with_test_hooks!(TestRuntime <= value_setter: ValueSetter<S>);

    #[test]
    // Tests the test setup by running the value setter module and checking if the value was set correctly
    fn test_value_setter_tx_success() {
        let value_to_set = 18;
        let assertion = Box::new(move |working_set: &mut WorkingSet<TestSpec>| {
            let value_setter = ValueSetter::<TestSpec>::default();
            let value = value_setter.value.get(working_set);
            assert_eq!(value, Some(value_to_set));
        });

        run_value_setter_txs_with_assertions(vec![(value_to_set, assertion)]);
    }

    #[test]
    #[should_panic]
    // Tests the test setup by running the value setter with an assertion that should fail and then trying to
    // run another transaction afterward. This would cause subsequent tests to block forever if the test runtime
    // failed to handle panics.
    fn test_value_setter_tx_bad_assertion() {
        let value_to_set = 18;
        let bad_assertion = Box::new(move |working_set: &mut WorkingSet<TestSpec>| {
            let value_setter = ValueSetter::<TestSpec>::default();
            let value = value_setter.value.get(working_set);
            assert_eq!(value, Some(value_to_set + 1)); // This will fail!
        });

        run_value_setter_txs_with_assertions(vec![
            (value_to_set, bad_assertion),
            (1, Box::new(|_| {})),
        ]);
    }

    // Sets a value and then runs the provided assertion
    fn run_value_setter_txs_with_assertions(
        values_and_assertions: Vec<(u32, WorkingSetClosure<TestRuntime<TestSpec, MockDaSpec>>)>,
    ) {
        let sequencer_rollup_addr = Address::from(SEQUENCER_ADDR);
        let admin_pkey = TestPrivateKey::generate();
        let admin_addr = (&admin_pkey.pub_key()).into();
        let genesis_config = create_test_rt_genesis_config(
            admin_addr,
            &[],
            sequencer_rollup_addr,
            SEQUENCER_ADDR.into(),
            100_000,
            "SovereignToken".to_string(),
            10_000_000_000,
            MockValidityCondChecker::default(),
        );
        let kernel_genesis = BasicKernelGenesisConfig {
            chain_state: ChainStateConfig {
                current_time: Default::default(),
                inner_code_commitment: MockCodeCommitment::default(),
                outer_code_commitment: MockCodeCommitment::default(),
                genesis_da_height: 0,
            },
        };
        let params = GenesisParams {
            runtime: genesis_config,
            kernel: kernel_genesis,
        };
        let txs_and_assertions = values_and_assertions
            .into_iter()
            .map(|(value, assertion)| {
                let msg = sov_value_setter::CallMessage::SetValue(value);
                let msg = <TestRuntime<TestSpec, MockDaSpec> as EncodeCall<
                    ValueSetter<TestSpec>,
                >>::encode_call(msg);

                let tx = Transaction::<TestSpec>::new_signed_tx(
                    &admin_pkey,
                    msg,
                    0,
                    1.into(),
                    100_000,
                    None,
                    0,
                );
                let tx = RawTx {
                    data: tx.try_to_vec().unwrap(),
                };
                (tx, assertion)
            })
            .collect::<Vec<_>>();

        run_test::<_, _>(
            params,
            txs_and_assertions,
            TestRuntime::<TestSpec, MockDaSpec>::default(),
        );
    }

    // TODO: generate this function in macro. We'll change the return type to a fixed `BasicGenesisConfig`
    // and then implement a helper function to combine this basic config with config for other modules to
    // create the full genesis config.
    //
    // This function should also take fewer arguments and generate data more aggressively.
    // <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/682>
    #[allow(clippy::too_many_arguments)]
    fn create_test_rt_genesis_config<S: Spec, Da: DaSpec>(
        admin: S::Address,
        additional_accounts: &[(S::Address, u64)],
        seq_rollup_address: S::Address,
        seq_da_address: Da::Address,
        seq_stake_amount: u64,
        token_name: String,
        init_balance: u64,
        validity_condition_checker: Da::Checker,
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
                minimum_attester_bond: MIN_USER_BOND,
                minimum_challenger_bond: MIN_USER_BOND,
                initial_attesters: vec![(admin.clone(), MIN_USER_BOND)],
                rollup_finality_period: ROLLUP_FINALITY_PERIOD,
                maximum_attested_height: MAX_ATTESTED_HEIGHT,
                light_client_finalized_height: LIGHT_CLIENT_FINALIZED_HEIGHT,
                validity_condition_checker,
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
}
