use std::str::FromStr;

pub use sov_bank::{Bank, BankConfig, Coins, TokenConfig, TokenId};
pub use sov_chain_state::ChainStateConfig;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::macros::DefaultRuntime;
use sov_modules_api::namespaces::Accessory;
use sov_modules_api::runtime::capabilities::{
    ContextResolver, GasEnforcer, TransactionDeduplicator,
};
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{
    Context, DaSpec, DispatchCall, Event, Gas, Genesis, MessageCodec, PublicKey, Spec,
    StateCheckpoint, StateReaderAndWriter, WorkingSet,
};
use sov_modules_stf_blueprint::{Runtime, SequencerOutcome};
pub use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
pub use sov_value_setter::{ValueSetter, ValueSetterConfig};
use tokio::sync::watch;

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
    pub bank: Bank<S>,
}

impl<S: Spec, Da: DaSpec> TxHooks for TestRuntime<S, Da> {
    type Spec = S;

    fn pre_dispatch_tx_hook(
        &self,
        _tx: &Transaction<Self::Spec>,
        _working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn post_dispatch_tx_hook(
        &self,
        _tx: &Transaction<Self::Spec>,
        _ctx: &Context<Self::Spec>,
        _working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

impl<S: Spec, Da: DaSpec> ApplyBatchHooks<Da> for TestRuntime<S, Da> {
    type Spec = S;
    type BatchResult = SequencerOutcome<Da::Address>;

    fn begin_batch_hook(
        &self,
        _batch: &mut BatchWithId,
        _sender: &<Da as DaSpec>::Address,
        _state_checkpoint: &mut StateCheckpoint<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn end_batch_hook(&self, _result: Self::BatchResult, _working_set: &mut StateCheckpoint<S>) {}
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
    type Tx = Transaction<S>;
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
        self.bank
            .refund_remaining_gas(tx, gas_meter, context.sender(), state_checkpoint);
    }
}

impl<S: Spec, Da: DaSpec> TransactionDeduplicator<S, Da> for TestRuntime<S, Da> {
    /// The transaction type that the deduplicator knows how to parse.
    type Tx = Transaction<S>;
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
}

/// Resolves the context for a transaction.
impl<S: Spec, Da: DaSpec> ContextResolver<S, Da> for TestRuntime<S, Da> {
    /// The transaction type that the resolver knows how to parse.
    type Tx = Transaction<S>;
    /// Resolves the context for a transaction.
    fn resolve_context(
        &self,
        tx: &Self::Tx,
        sequencer: &Da::Address,
        height: u64,
        working_set: &mut StateCheckpoint<S>,
    ) -> Context<S> {
        let sender = tx.pub_key().to_address();
        let sequencer = self
            .sequencer_registry
            .resolve_da_address(sequencer, working_set)
            .expect("Sequencer is no longer registered by the time of context resolution. This is a bug");
        Context::new(sender, sequencer, height)
    }
}

/// Admin: single address that will be used as admin, minter and sequencer
pub fn create_genesis_config<S: Spec, Da: DaSpec>(
    admin: S::Address,
    admin_da_address: Da::Address,
    seq_stake_amount: u64,
    token_name: String,
    init_balance: u64,
) -> GenesisConfig<S, Da> {
    assert!(
        init_balance >= seq_stake_amount,
        "sequencer cannot stake more than its initial balance"
    );
    let token_id = TokenId::from_str(sov_bank::GAS_TOKEN_ID).expect("failed to parse token id");
    GenesisConfig {
        value_setter: ValueSetterConfig {
            admin: admin.clone(),
        },
        sequencer_registry: SequencerConfig {
            seq_rollup_address: admin.clone(),
            seq_da_address: admin_da_address,
            coins_to_lock: Coins {
                amount: seq_stake_amount,
                token_id,
            },
            is_preferred_sequencer: true,
        },
        bank: BankConfig {
            gas_token_config: sov_bank::GasTokenConfig {
                token_name: token_name.clone(),
                address_and_balances: vec![(admin.clone(), init_balance)],
                authorized_minters: vec![admin.clone()],
            },
            tokens: vec![],
        },
    }
}
