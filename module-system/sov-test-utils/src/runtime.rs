use std::marker::PhantomData;

use sov_attester_incentives::{AttesterIncentives, AttesterIncentivesConfig};
use sov_bank::IntoPayable;
pub use sov_bank::{Bank, BankConfig, Coins, TokenConfig, TokenId};
pub use sov_chain_state::ChainStateConfig;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::macros::DefaultRuntime;
use sov_modules_api::namespaces::Accessory;
use sov_modules_api::runtime::capabilities::{
    AuthenticationError, GasEnforcer, RawTx, RuntimeAuthenticator, RuntimeAuthorization,
};
use sov_modules_api::transaction::{
    AuthenticatedTransactionAndRawHash, AuthenticatedTransactionData,
};
use sov_modules_api::{
    Context, DaSpec, DispatchCall, Event, Gas, Genesis, MessageCodec, ModuleInfo, Spec,
    StateCheckpoint, StateReaderAndWriter, WorkingSet,
};
use sov_modules_stf_blueprint::{Runtime, SequencerOutcome};
pub use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
pub use sov_value_setter::{ValueSetter, ValueSetterConfig};
use tokio::sync::watch;

const MIN_USER_BOND: u64 = 10;
const MAX_ATTESTED_HEIGHT: u64 = 0;
const LIGHT_CLIENT_FINALIZED_HEIGHT: u64 = 0;
const ROLLUP_FINALITY_PERIOD: u64 = 1;

#[derive(Genesis, DispatchCall, Event, MessageCodec, DefaultRuntime)]
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

    fn pre_dispatch_tx_hook(
        &self,
        _tx: &AuthenticatedTransactionData<Self::Spec>,
        _working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn post_dispatch_tx_hook(
        &self,
        _tx: &AuthenticatedTransactionData<Self::Spec>,
        _ctx: &Context<Self::Spec>,
        _working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

impl<S: Spec, Da: DaSpec> ApplyBatchHooks<Da> for TestRuntime<S, Da> {
    type Spec = S;
    type BatchResult = SequencerOutcome;

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
            SequencerOutcome::Rewarded(amount) => {
                sov_sequencer_registry::SequencerOutcome::Rewarded(amount)
            }
            SequencerOutcome::Ignored => sov_sequencer_registry::SequencerOutcome::Ignored,
            SequencerOutcome::Slashed(_reason) => sov_sequencer_registry::SequencerOutcome::Slashed,
            SequencerOutcome::Penalized(amount) => {
                sov_sequencer_registry::SequencerOutcome::Penalized(amount)
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

    fn begin_slot_hook(
        &self,
        _pre_state_root: <Self::Spec as Spec>::VisibleHash,
        _working_set: &mut sov_modules_api::VersionedStateReadWriter<StateCheckpoint<S>>,
    ) {
    }

    fn end_slot_hook(&self, _working_set: &mut StateCheckpoint<S>) {}
}

impl<S: Spec, Da: DaSpec> FinalizeHook for TestRuntime<S, Da> {
    type Spec = S;

    fn finalize_hook(
        &self,
        _root_hash: <Self::Spec as Spec>::VisibleHash,
        _accessory_working_set: &mut impl StateReaderAndWriter<Accessory>,
    ) {
    }
}

impl<S: Spec, Da: DaSpec> RuntimeAuthenticator for TestRuntime<S, Da> {
    type Decodable = <Self as DispatchCall>::Decodable;

    type Tx = AuthenticatedTransactionAndRawHash<S>;

    fn authenticate(
        &self,
        raw_tx: &RawTx,
    ) -> Result<(Self::Tx, Self::Decodable), AuthenticationError> {
        sov_modules_api::authenticate::<S, Self>(&raw_tx.data)
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
    /// The transaction type that the gas enforcer knows how to parse
    type Tx = AuthenticatedTransactionData<S>;
    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas(
        &self,
        tx: &Self::Tx,
        context: &Context<S>,
        gas_price: &<S::Gas as Gas>::Price,
        mut state_checkpoint: StateCheckpoint<S>,
    ) -> Result<WorkingSet<S>, StateCheckpoint<S>> {
        match self
            .bank
            .reserve_gas(tx, gas_price, context.sender(), &mut state_checkpoint)
        {
            Ok(gas_meter) => Ok(state_checkpoint.to_revertable(gas_meter)),
            Err(e) => {
                tracing::debug!("Unable to reserve gas from {}. {}", e, context.sender());
                Err(state_checkpoint)
            }
        }
    }

    /// Refunds any remaining gas to the payer after the transaction is processed.
    fn refund_remaining_gas(
        &self,
        tx: &Self::Tx,
        context: &Context<S>,
        gas_meter: &sov_modules_api::GasMeter<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.bank.refund_remaining_gas(
            tx,
            gas_meter,
            context.sender(),
            &self.attester_incentives.id().to_payable(),
            &self.sequencer_registry.id().to_payable(),
            state_checkpoint,
        );
    }
}

impl<S: Spec, Da: DaSpec> RuntimeAuthorization<S, Da> for TestRuntime<S, Da> {
    type Tx = AuthenticatedTransactionData<S>;
    /// Prevents duplicate transactions from running.
    // TODO(@preston-evans98): Use type system to prevent writing to the `StateCheckpoint` during this check
    fn check_uniqueness(
        &self,
        _tx: &Self::Tx,
        _context: &Context<S>,
        _state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        _tx: &Self::Tx,
        _sequencer: &Da::Address,
        _state_checkpoint: &mut StateCheckpoint<S>,
    ) {
    }

    /// Resolves the context for a transaction.
    fn resolve_context(
        &self,
        tx: &Self::Tx,
        sequencer: &Da::Address,
        height: u64,
        working_set: &mut StateCheckpoint<S>,
    ) -> Context<S> {
        let sender = tx.default_address().clone();
        let sequencer = self
            .sequencer_registry
            .resolve_da_address(sequencer, working_set)
            .expect("Sequencer is no longer registered by the time of context resolution. This is a bug");
        Context::new(sender, sequencer, height)
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
