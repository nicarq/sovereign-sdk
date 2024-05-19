use std::marker::PhantomData;

use sov_attester_incentives::{AttesterIncentives, AttesterIncentivesConfig};
pub use sov_bank::{Bank, BankConfig, Coins, TokenConfig, TokenId};
use sov_bank::{IntoPayable, ReserveGasError};
pub use sov_chain_state::ChainStateConfig;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::runtime::capabilities::{
    AuthenticationError, GasEnforcer, RawTx, RuntimeAuthenticator, RuntimeAuthorization,
    SequencerAuthorization,
};
use sov_modules_api::transaction::{
    AuthenticatedTransactionAndRawHash, AuthenticatedTransactionData,
};
use sov_modules_api::{
    Context, DaSpec, DispatchCall, Event, Gas, Genesis, MessageCodec, ModuleInfo, Spec,
    StateCheckpoint, TransactionConsumption, WorkingSet,
};
use sov_modules_stf_blueprint::{BatchSequencerOutcome, Runtime};
use sov_sequencer_registry::SequencerStakeMeter;
pub use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
pub use sov_value_setter::{ValueSetter, ValueSetterConfig};
use tokio::sync::watch;

const MIN_USER_BOND: u64 = 10;
const MAX_ATTESTED_HEIGHT: u64 = 0;
const LIGHT_CLIENT_FINALIZED_HEIGHT: u64 = 0;
const ROLLUP_FINALITY_PERIOD: u64 = 1;

#[derive(Default, Genesis, DispatchCall, Event, MessageCodec)]
#[serialization(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize
)]
pub struct TestRuntime<S: Spec, Da: DaSpec> {
    pub value_setter: ValueSetter<S>,
    pub sequencer_registry: SequencerRegistry<S, Da>,
    pub attester_incentives: AttesterIncentives<S, Da>,
    pub bank: Bank<S>,
}

impl<S: Spec, Da: DaSpec> TxHooks for TestRuntime<S, Da> {
    type Spec = S;
}

impl<S: Spec, Da: DaSpec> ApplyBatchHooks<Da> for TestRuntime<S, Da> {
    type Spec = S;
    type BatchResult = BatchSequencerOutcome;

    fn begin_batch_hook(
        &self,
        batch: &mut BatchWithId,
        sender: &<Da as DaSpec>::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> anyhow::Result<()> {
        self.sequencer_registry
            .begin_batch_hook(batch, sender, state_checkpoint)
    }

    fn end_batch_hook(
        &self,
        result: Self::BatchResult,
        sender: &<Da as DaSpec>::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        // Since we need to make sure the `StfBlueprint` doesn't depend on the module system, we need to
        // convert the `SequencerOutcome` structures manually.
        let seqencer_outcome = match result {
            BatchSequencerOutcome::Rewarded(amount) => {
                sov_sequencer_registry::SequencerOutcome::Rewarded(amount.into())
            }
            BatchSequencerOutcome::Ignored => sov_sequencer_registry::SequencerOutcome::Ignored,
            BatchSequencerOutcome::Slashed(_reason) => {
                sov_sequencer_registry::SequencerOutcome::Slashed
            }
        };

        <SequencerRegistry<S, Da> as ApplyBatchHooks<Da>>::end_batch_hook(
            &self.sequencer_registry,
            seqencer_outcome,
            sender,
            state_checkpoint,
        );
    }
}

impl<S: Spec, Da: DaSpec> SlotHooks for TestRuntime<S, Da> {
    type Spec = S;
}

impl<S: Spec, Da: DaSpec> FinalizeHook for TestRuntime<S, Da> {
    type Spec = S;
}

impl<S: Spec, Da: DaSpec> RuntimeAuthenticator<S> for TestRuntime<S, Da> {
    type Decodable = <Self as DispatchCall>::Decodable;

    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    fn authenticate(
        &self,
        raw_tx: &RawTx,
        sequencer_stake_meter: &mut Self::SequencerStakeMeter,
    ) -> Result<(AuthenticatedTransactionAndRawHash<S>, Self::Decodable), AuthenticationError> {
        sov_modules_api::authenticate::<S, Self>(&raw_tx.data, sequencer_stake_meter)
    }
}

impl<S: Spec, Da: DaSpec> Runtime<S, Da> for TestRuntime<S, Da> {
    type GenesisConfig = GenesisConfig<S, Da>;

    type GenesisPaths = ();

    fn rpc_methods(_storage: watch::Receiver<S::Storage>) -> jsonrpsee::RpcModule<()> {
        todo!()
    }

    fn genesis_config(
        _genesis_paths: &Self::GenesisPaths,
    ) -> Result<Self::GenesisConfig, anyhow::Error> {
        todo!()
    }
}

impl<S: Spec, Da: DaSpec> GasEnforcer<S, Da> for TestRuntime<S, Da> {
    /// A type that tracks the gas consumed by pre-execution checks
    type PreExecChecksMeter = SequencerStakeMeter<S::Gas>;

    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        context: &Context<S>,
        gas_price: &<S::Gas as Gas>::Price,
        pre_exec_checks_meter: &Self::PreExecChecksMeter,
        state_checkpoint: StateCheckpoint<S>,
    ) -> Result<WorkingSet<S>, StateCheckpoint<S>> {
        self.bank
            .reserve_gas(
                tx,
                gas_price,
                context.sender(),
                pre_exec_checks_meter,
                state_checkpoint,
            )
            .map_err(
                |ReserveGasError {
                     state_checkpoint,
                     reason,
                 }| {
                    tracing::debug!(
                        "Unable to reserve gas from {}. {}",
                        reason,
                        context.sender()
                    );
                    state_checkpoint
                },
            )
    }

    fn allocate_consumed_gas(
        &self,
        consumption: &TransactionConsumption<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.bank.allocate_consumed_gas(
            &self.attester_incentives.id().to_payable(),
            &self.sequencer_registry.id().to_payable(),
            consumption,
            state_checkpoint,
        );
    }

    fn refund_remaining_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        context: &Context<S>,
        consumption: &TransactionConsumption<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.bank
            .refund_remaining_gas(tx, context.sender(), consumption, state_checkpoint);
    }
}

impl<S: Spec, Da: DaSpec> SequencerAuthorization<S, Da> for TestRuntime<S, Da> {
    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    fn authorize_sequencer(
        &self,
        sequencer: &<Da as DaSpec>::Address,
        base_fee_per_gas: &<S::Gas as Gas>::Price,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<Self::SequencerStakeMeter, anyhow::Error> {
        self.sequencer_registry
            .authorize_sequencer(sequencer, base_fee_per_gas, state_checkpoint)
            .map_err(|e| {
                anyhow::anyhow!("An error occurred while checking the sequencer bond: {e}")
            })
    }

    fn refund_sequencer(
        &self,
        sequencer_stake_meter: &mut Self::SequencerStakeMeter,
        refund_amount: u64,
    ) {
        self.sequencer_registry
            .refund_sequencer(sequencer_stake_meter, refund_amount);
    }

    fn penalize_sequencer(
        &self,
        sequencer: &Da::Address,
        sequencer_stake_meter: Self::SequencerStakeMeter,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.sequencer_registry.penalize_sequencer(
            sequencer,
            sequencer_stake_meter,
            state_checkpoint,
        );
    }
}

impl<S: Spec, Da: DaSpec> RuntimeAuthorization<S, Da> for TestRuntime<S, Da> {
    /// Prevents duplicate transactions from running.
    // TODO(@preston-evans98): Use type system to prevent writing to the `StateCheckpoint` during this check
    fn check_uniqueness(
        &self,
        _tx: &AuthenticatedTransactionData<S>,
        _context: &Context<S>,
        _state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        _tx: &AuthenticatedTransactionData<S>,
        _sequencer: &Da::Address,
        _state_checkpoint: &mut StateCheckpoint<S>,
    ) {
    }

    /// Resolves the context for a transaction.
    fn resolve_context(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        sequencer: &Da::Address,
        height: u64,
        working_set: &mut StateCheckpoint<S>,
    ) -> Result<Context<S>, anyhow::Error> {
        let sender = tx.default_address.clone().unwrap();
        let sequencer = self
            .sequencer_registry
            .resolve_da_address(sequencer, working_set)
            .expect("Sequencer is no longer registered by the time of context resolution. This is a bug");
        Ok(Context::new(sender, sequencer, height))
    }
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
