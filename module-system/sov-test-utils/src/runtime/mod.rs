use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::PathBuf;

use borsh::BorshSerialize;
pub use sov_attester_incentives;
pub use sov_attester_incentives::{
    AttesterIncentives, AttesterIncentivesConfig, CallMessage as AttesterCallMessage,
};
pub use sov_bank::{Bank, BankConfig, Coins, TokenConfig, TokenId};
pub use sov_chain_state::ChainStateConfig;
use sov_db::schema::SchemaBatch;
pub use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockBlob, MockBlock, MockBlockHeader, MockDaSpec};
use sov_modules_api::hooks::TxHooks;
use sov_modules_api::macros::config_value;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{
    Batch, BlobData, CryptoSpec, DaSpec, EncodeCall, Genesis, Module, PrivateKey, RawTx, SlotData,
    Spec, StateCheckpoint, WorkingSet,
};
pub use sov_modules_stf_blueprint::GenesisParams;
use sov_modules_stf_blueprint::{Runtime, StfBlueprint};
use sov_prover_storage_manager::ProverStorageManager;
use sov_rollup_interface::da::RelevantBlobIters;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
pub use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
use sov_state::{DefaultStorageSpec, ProverStorage, Storage};
pub use sov_value_setter::{ValueSetter, ValueSetterConfig};

pub mod genesis;
pub mod traits;
pub mod wrapper;
use traits::{MinimalGenesis, PostTxHookRegistry};
pub use wrapper::{TestRuntimeWrapper, WorkingSetClosure};

// Constants used in the genesis configuration of the test runtime
const MIN_USER_BOND: u64 = 100_000;
const MAX_ATTESTED_HEIGHT: u64 = 0;
const LIGHT_CLIENT_FINALIZED_HEIGHT: u64 = 0;
const ROLLUP_FINALITY_PERIOD: u64 = 1;

/// Generates a runtime containing the [`Bank`], [`AttesterIncentives`](sov_attester_incentives::AttesterIncentives),
/// and [`SequencerRegistry`] modules in addition to any provided as arguments`
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

            fn attester_incentives(&self) -> &$crate::runtime::AttesterIncentives<S, Da> {
                &self.attester_incentives
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
            fn sequencer_registry_config(config: &mut GenesisConfig<S, Da>) -> &mut <$crate::runtime::SequencerRegistry<S, Self::Da> as ::sov_modules_api::Genesis>::Config {
                &mut config.sequencer_registry
            }

            fn bank_config(config: &mut GenesisConfig<S, Da>) -> &mut <$crate::runtime::Bank<S> as ::sov_modules_api::Genesis>::Config {
                &mut config.bank
            }

            fn attester_incentives_config(config: &mut GenesisConfig<S, Da>) -> &mut <$crate::runtime::AttesterIncentives<S, Self::Da> as ::sov_modules_api::Genesis>::Config {
                &mut config.attester_incentives
            }
        }



        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> GenesisConfig<S, Da> {
            #[allow(unused)]
            pub fn from_minimal_config(minimal_config: $crate::runtime::genesis::MinimalOptimisticGenesisConfig<S, Da>,
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

type DefaultSpecWithHasher<S> = DefaultStorageSpec<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>;

pub struct SlotTestCase<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> {
    pub transaction_test_cases: Vec<TxTestCase<RT, M, S>>,
    pub post_hook: EndSlotClosure<StateCheckpoint<S>>,
}

impl<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> SlotTestCase<RT, M, S> {
    pub fn empty() -> Self {
        Self {
            transaction_test_cases: vec![],
            post_hook: Box::new(|_| {}),
        }
    }

    pub fn from_txs(test_cases: Vec<TxTestCase<RT, M, S>>) -> Self {
        Self {
            transaction_test_cases: test_cases,
            post_hook: Box::new(|_| {}),
        }
    }
}

impl<T: Into<TxTestCase<RT, M, S>>, RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> From<Vec<T>>
    for SlotTestCase<RT, M, S>
{
    fn from(test_cases: Vec<T>) -> Self {
        SlotTestCase {
            transaction_test_cases: test_cases.into_iter().map(Into::into).collect(),
            post_hook: Box::new(|_| {}),
        }
    }
}

impl<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec>
    From<(
        <S::CryptoSpec as CryptoSpec>::PrivateKey,
        WorkingSetClosure<RT>,
        <M as Module>::CallMessage,
    )> for TxTestCase<RT, M, S>
{
    fn from(
        (sender_key, post_check, message): (
            <S::CryptoSpec as CryptoSpec>::PrivateKey,
            WorkingSetClosure<RT>,
            <M as Module>::CallMessage,
        ),
    ) -> Self {
        TxTestCase {
            outcome: TxOutcome::Applied(post_check),
            message: MessageType::Plain(message, sender_key),
        }
    }
}

pub enum TxOutcome<RT: TxHooks> {
    /// Expects that the tx was successful and runs the provided closure in the post_dispatch hook
    Applied(WorkingSetClosure<RT>),
    /// Expects that the tx was reverted
    Reverted,
}

impl<RT: TxHooks> TxOutcome<RT> {
    pub fn applied() -> Self {
        Self::Applied(Box::new(|_| {}))
    }
}

pub enum MessageType<M: Module, S: Spec> {
    PreSigned(RawTx),
    PreEncoded(Vec<u8>, <S::CryptoSpec as CryptoSpec>::PrivateKey),
    Plain(M::CallMessage, <S::CryptoSpec as CryptoSpec>::PrivateKey),
}

impl<M: Module, S: Spec> MessageType<M, S> {
    pub fn to_raw_tx<RT: EncodeCall<M>>(
        self,
        nonces: &mut HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    ) -> RawTx {
        match self {
            MessageType::PreSigned(raw_tx) => raw_tx,
            MessageType::PreEncoded(msg, key) => Self::sign_with_defaults(msg, key, nonces),
            MessageType::Plain(msg, key) => {
                let msg = <RT as EncodeCall<M>>::encode_call(msg);
                Self::sign_with_defaults(msg, key, nonces)
            }
        }
    }

    pub fn pre_signed(
        unsigned_tx: UnsignedTransaction<S>,
        key: &<S::CryptoSpec as CryptoSpec>::PrivateKey,
    ) -> Self {
        let tx = Transaction::new_signed_tx(key, unsigned_tx)
            .try_to_vec()
            .unwrap();
        Self::PreSigned(RawTx { data: tx })
    }

    pub fn sign_with_defaults(
        msg: Vec<u8>,
        key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
        nonces: &mut HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    ) -> RawTx {
        let pub_key = key.pub_key();
        let nonce = *nonces.get(&pub_key).unwrap_or(&0);
        nonces.insert(pub_key, nonce + 1);
        let tx = Transaction::<S>::new_signed_tx(
            &key,
            UnsignedTransaction::new(
                msg,
                config_value!("CHAIN_ID"),
                PriorityFeeBips::ZERO,
                10_000_000,
                nonce,
                None,
            ),
        )
        .try_to_vec()
        .unwrap();

        RawTx { data: tx }
    }
}

pub struct TxTestCase<RT: Runtime<S, MockDaSpec>, M: Module, S: Spec> {
    pub outcome: TxOutcome<RT>,
    pub message: MessageType<M, S>,
}

/// Run a test on the given runtime
///
/// The test is defined by a series of slot test cases, where the workflow is...
/// 1. Run genesis
/// 2. For each call message, execute the message and apply the post-execution closure to check
/// that the result is valid.
pub fn run_test<RT, S, M>(
    genesis_config: GenesisParams<<RT as Genesis>::Config, BasicKernelGenesisConfig<S, MockDaSpec>>,
    slots: Vec<SlotTestCase<RT, M, S>>,
    runtime: RT,
) where
    RT: Runtime<S, MockDaSpec>
        + PostTxHookRegistry<S, MockDaSpec>
        + EndSlotHookRegistry<S, MockDaSpec>
        + MinimalGenesis<S, Da = MockDaSpec>
        + EncodeCall<M>,
    S: Spec<Storage = ProverStorage<DefaultSpecWithHasher<S>>>,
    M: Module,
{
    run_test_with_setup_fn(genesis_config, &mut |_, _, _| {}, slots, runtime);
}

/// Run a test on the given runtime
///
/// The test is defined by a series of slot test cases, where the workflow is...
/// 1. Run genesis
/// 2. For each slot, apply the provided pre-execution closure to each call message
/// with the current state as an argument. This allows us to set update any call messages
/// that depend on the current state.
/// 3. For each call message, execute the message and apply the post-execution closure to check
/// that the result is valid.
pub fn run_test_with_setup_fn<RT, S, M>(
    mut genesis_config: GenesisParams<
        <RT as Genesis>::Config,
        BasicKernelGenesisConfig<S, MockDaSpec>,
    >,
    tx_setup_fn: &mut StateRootClosure<
        <M as Module>::CallMessage,
        <<S as Spec>::Storage as Storage>::Root,
        <RT as TxHooks>::TxState,
    >,
    slots: Vec<SlotTestCase<RT, M, S>>,
    runtime: RT,
) where
    RT: Runtime<S, MockDaSpec>
        + PostTxHookRegistry<S, MockDaSpec>
        + EndSlotHookRegistry<S, MockDaSpec>
        + MinimalGenesis<S, Da = MockDaSpec>
        + EncodeCall<M>,
    S: Spec<Storage = ProverStorage<DefaultSpecWithHasher<S>>>,
    M: Module,
{
    let mut nonces = HashMap::new();
    let mut post_slot_closures = Vec::with_capacity(slots.len());
    let mut messages_by_slot = Vec::with_capacity(slots.len());
    let mut tx_successful = Vec::new();
    // Register the transaction hooks with the runtime. Destructure the test cases for easier processing.
    {
        for slot in slots {
            let SlotTestCase {
                transaction_test_cases,
                post_hook,
            } = slot;
            post_slot_closures.push(post_hook);

            let mut messages = Vec::with_capacity(transaction_test_cases.len());
            let mut hooks = Vec::with_capacity(transaction_test_cases.len());
            for test_case in transaction_test_cases {
                let TxTestCase { outcome, message } = test_case;
                messages.push(message);
                if let TxOutcome::Applied(post_check) = outcome {
                    hooks.push(post_check);
                    tx_successful.push(true);
                } else {
                    tx_successful.push(false);
                };
            }
            runtime.add_post_dispatch_tx_hook_actions(hooks);
            messages_by_slot.push(messages);
        }
    }
    runtime.add_end_slot_hook_actions(post_slot_closures);

    // Use the runtime to create an STF blueprint
    let stf = StfBlueprint::<S, MockDaSpec, RT, BasicKernel<S, MockDaSpec>>::with_runtime(runtime);

    // ----- Setup and run genesis ---------
    let temp_dir = tempfile::tempdir().unwrap();
    let storage_config = sov_state::config::Config {
        path: PathBuf::from(temp_dir.path()),
    };
    let sequencer_da_address =
        <RT as MinimalGenesis<S>>::sequencer_registry_config(&mut genesis_config.runtime)
            .seq_da_address;

    let mut storage_manager = ProverStorageManager::<MockDaSpec, _>::new(storage_config)
        .expect("ProverStorageManager initialization has failed");

    let genesis_block = MockBlock::default();
    let (stf_state, _) = storage_manager
        .create_state_for(genesis_block.header())
        .unwrap();
    let (state_root, change_set) = stf.init_chain(stf_state, genesis_config);

    storage_manager
        .save_change_set(genesis_block.header(), change_set, SchemaBatch::new())
        .unwrap();
    // Write it to the database immediately
    storage_manager.finalize(&genesis_block.header).unwrap();
    let mut prev_state_root = state_root;
    // ----- End genesis ---------

    let mut expect_success = tx_successful.into_iter();
    for (prev_slot_number, msgs_and_priv_keys) in messages_by_slot.into_iter().enumerate() {
        let block_header = MockBlockHeader::from_height(prev_slot_number as u64 + 1);
        let (stf_state, _) = storage_manager
            .create_state_for(&block_header)
            .expect("Block builds on height zero");
        // Setup call messages
        let txs = {
            let mut state = WorkingSet::<S>::new(stf_state.clone());
            let mut signed_txs = Vec::new();

            for mut msg in msgs_and_priv_keys.into_iter() {
                if let MessageType::Plain(msg, _) = &mut msg {
                    tx_setup_fn(msg, prev_state_root, &mut state);
                }

                signed_txs.push(msg.to_raw_tx::<RT>(&mut nonces));
            }
            signed_txs
        };

        let batch = BlobData::Batch(Batch { txs });
        let blob = batch.try_to_vec().unwrap();
        let mut blob = MockBlob::new_with_hash(blob, sequencer_da_address);

        let relevant_blobs = RelevantBlobIters {
            proof_blobs: vec![],
            batch_blobs: vec![&mut blob],
        };
        let result = stf.apply_slot(
            &state_root,
            stf_state,
            Default::default(),
            &block_header,
            &Default::default(),
            relevant_blobs,
        );
        for batch in result.batch_receipts {
            for tx_receipt in batch.tx_receipts {
                if expect_success
                    .next()
                    .expect("Must have one outcome per transaction")
                {
                    assert!(tx_receipt.receipt.is_successful());
                } else {
                    assert!(tx_receipt.receipt.is_reverted());
                }
            }
        }

        storage_manager
            .save_change_set(&block_header, result.change_set, SchemaBatch::new())
            .unwrap();
        prev_state_root = result.state_root;
    }

    assert!(
        stf.runtime().try_get_next_tx_action().flatten().is_none(),
        "All post tx hooks must have run! This error indicates that at least one transaction failed that was expected to succeed!"
    );

    assert!(
        stf.runtime().try_get_next_slot_action().flatten().is_none(),
        "All end slot hooks must have run! This should be unreachable!"
    );
}

// TODO: Delete the hookless TestRuntime after upgrading tests to the HookedRuntime
// <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/682>
generate_optimistic_runtime!(TestRuntime <= value_setter: ValueSetter<S>);

use self::traits::EndSlotHookRegistry;
use self::wrapper::{EndSlotClosure, StateRootClosure};

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
            minimum_attester_bond: MIN_USER_BOND,
            minimum_challenger_bond: MIN_USER_BOND,
            initial_attesters: vec![(admin.clone(), MIN_USER_BOND)],
            rollup_finality_period: ROLLUP_FINALITY_PERIOD,
            maximum_attested_height: MAX_ATTESTED_HEIGHT,
            light_client_finalized_height: LIGHT_CLIENT_FINALIZED_HEIGHT,
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
    use sov_mock_da::MockDaSpec;
    use sov_mock_zkvm::MockCodeCommitment;
    use sov_modules_api::{Address, PrivateKey, WorkingSet};
    use sov_modules_stf_blueprint::GenesisParams;

    use super::*;
    use crate::{TestPrivateKey, TestSpec};

    const SEQUENCER_ADDR: [u8; 32] = [42u8; 32];
    generate_optimistic_runtime!(TestRuntime <= value_setter: ValueSetter<S>);

    #[test]
    // Tests the test setup by running the value setter module and checking if the value was set correctly
    fn test_value_setter_tx_success() {
        let value_to_set = 18;
        let assertion = Box::new(move |state: &mut WorkingSet<TestSpec>| {
            let value_setter = ValueSetter::<TestSpec>::default();
            let value = value_setter
                .value
                .get(state)
                .expect("We should be able to get a value from the state");
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
        let bad_assertion = Box::new(move |state: &mut WorkingSet<TestSpec>| {
            let value_setter = ValueSetter::<TestSpec>::default();
            let value = value_setter
                .value
                .get(state)
                .expect("We should be able to get a value from the state");
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
        let tx_test_cases = values_and_assertions
            .into_iter()
            .map(|(value, assertion)| {
                let msg = sov_value_setter::CallMessage::SetValue(value);
                TxTestCase::<_, ValueSetter<TestSpec>, _>::from((
                    admin_pkey.clone(),
                    assertion,
                    msg,
                ))
            })
            .collect::<Vec<_>>();

        run_test::<_, _, _>(
            params,
            vec![SlotTestCase::from(tx_test_cases)],
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
