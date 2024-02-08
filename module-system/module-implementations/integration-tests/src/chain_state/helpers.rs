use std::sync::{Arc, RwLock};

use sov_bank::{get_genesis_token_address, Bank, BankConfig, Coins, TokenConfig};
use sov_chain_state::ChainStateConfig;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::macros::DefaultRuntime;
use sov_modules_api::runtime::capabilities::{
    ContextResolver, GasEnforcer, Kernel, TransactionDeduplicator,
};
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{
    AccessoryStateCheckpoint, Context, DaSpec, DispatchCall, Event, GasUnit, Genesis, MessageCodec,
    PublicKey, Spec, StateCheckpoint, WorkingSet,
};
use sov_modules_stf_blueprint::kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_modules_stf_blueprint::{GenesisParams, Runtime, SequencerOutcome};
use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
use sov_state::Storage;
use sov_value_setter::{ValueSetter, ValueSetterConfig};

#[derive(Genesis, DispatchCall, Event, MessageCodec, DefaultRuntime)]
#[serialization(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize
)]
pub(crate) struct TestRuntime<C: Context, Da: DaSpec> {
    pub value_setter: ValueSetter<C>,
    pub sequencer_registry: SequencerRegistry<C, Da>,
    pub bank: Bank<C>,
}

pub(crate) fn create_chain_state_genesis_config<C: Context, Da: DaSpec>(
    admin_pub_key: <C as Spec>::Address,
    seq_rollup_address: <C as Spec>::Address,
    seq_da_address: Da::Address,
    seq_stake_amount: u64,
    token_name: String,
    salt: u64,
    init_balance: u64,
) -> GenesisParams<GenesisConfig<C, Da>, BasicKernelGenesisConfig<C, Da>> {
    let runtime_config: <TestRuntime<C, Da> as sov_modules_stf_blueprint::Runtime<C, Da>>::GenesisConfig =
        GenesisConfig { value_setter: ValueSetterConfig { admin: admin_pub_key }, sequencer_registry: SequencerConfig{
            seq_rollup_address: seq_rollup_address.clone(),
            seq_da_address,
            coins_to_lock: Coins { amount: seq_stake_amount, token_address: get_genesis_token_address::<C>(&token_name, salt) },
            is_preferred_sequencer: true,
        }, bank: BankConfig{
            tokens: vec![TokenConfig{token_name,
            address_and_balances: vec![(seq_rollup_address.clone(), init_balance)], authorized_minters: vec![seq_rollup_address.clone()], salt}]
        } };

    let kernel_config: <TestKernel<C, Da> as Kernel<C, Da>>::GenesisConfig =
        BasicKernelGenesisConfig {
            chain_state: ChainStateConfig {
                current_time: Default::default(),
                gas_price_blocks_depth: 10,
                gas_price_maximum_elasticity: 1,
                initial_gas_price: GasUnit::ZEROED,
                minimum_gas_price: GasUnit::ZEROED,
            },
        };
    GenesisParams {
        runtime: runtime_config,
        kernel: kernel_config,
    }
}

pub(crate) type TestKernel<C, Da> = BasicKernel<C, Da>;

impl<C: Context, Da: DaSpec> TxHooks for TestRuntime<C, Da> {
    type Context = C;

    fn pre_dispatch_tx_hook(
        &self,
        _tx: &Transaction<Self::Context>,
        _working_set: &mut sov_modules_api::WorkingSet<C>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn post_dispatch_tx_hook(
        &self,
        _tx: &Transaction<Self::Context>,
        _ctx: &C,
        _working_set: &mut sov_modules_api::WorkingSet<C>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

impl<C: Context, Da: DaSpec> ApplyBatchHooks<Da> for TestRuntime<C, Da> {
    type Context = C;
    type BatchResult = SequencerOutcome<Da::Address>;

    fn begin_batch_hook(
        &self,
        _batch: &mut BatchWithId,
        _sender: &<Da as DaSpec>::Address,
        _state_checkpoint: &mut sov_modules_api::StateCheckpoint<C>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn end_batch_hook(
        &self,
        _result: Self::BatchResult,
        _working_set: &mut sov_modules_api::StateCheckpoint<C>,
    ) {
    }
}

impl<C: Context, Da: DaSpec> SlotHooks for TestRuntime<C, Da> {
    type Context = C;

    fn begin_slot_hook(
        &self,
        _pre_state_root: &<<Self::Context as Spec>::Storage as Storage>::Root,
        _working_set: &mut sov_modules_api::VersionedStateReadWriter<StateCheckpoint<C>>,
    ) {
    }

    fn end_slot_hook(&self, _working_set: &mut sov_modules_api::StateCheckpoint<C>) {}
}

impl<C: Context, Da: DaSpec> FinalizeHook for TestRuntime<C, Da> {
    type Context = C;

    fn finalize_hook(
        &self,
        _root_hash: &<<Self::Context as Spec>::Storage as Storage>::Root,
        _accesorry_working_set: &mut AccessoryStateCheckpoint<C>,
    ) {
    }
}

impl<C: Context, Da: DaSpec> Runtime<C, Da> for TestRuntime<C, Da> {
    type GenesisConfig = GenesisConfig<C, Da>;

    type GenesisPaths = ();

    fn rpc_methods(_storage: Arc<RwLock<<C as Spec>::Storage>>) -> jsonrpsee::RpcModule<()> {
        todo!()
    }

    fn genesis_config(
        _genesis_paths: &Self::GenesisPaths,
    ) -> Result<Self::GenesisConfig, anyhow::Error> {
        todo!()
    }
}

impl<C: Context, Da: DaSpec> GasEnforcer<C, Da> for TestRuntime<C, Da> {
    /// The transaction type that the gas enforcer knows how to parse
    type Tx = Transaction<C>;
    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas(
        &self,
        tx: &Self::Tx,
        context: &C,
        gas_price: &C::GasUnit,
        mut state_checkpoint: StateCheckpoint<C>,
    ) -> Result<WorkingSet<C>, StateCheckpoint<C>> {
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
        context: &C,
        gas_meter: &sov_modules_api::GasMeter<C::GasUnit>,
        state_checkpoint: &mut StateCheckpoint<C>,
    ) {
        self.bank
            .refund_remaining_gas(tx, gas_meter, context.sender(), state_checkpoint);
    }
}

impl<C: Context, Da: DaSpec> TransactionDeduplicator<C, Da> for TestRuntime<C, Da> {
    /// The transaction type that the deduplicator knows how to parse.
    type Tx = Transaction<C>;
    /// Prevents duplicate transactions from running.
    // TODO(@preston-evans98): Use type system to prevent writing to the `StateCheckpoint` during this check
    fn check_uniqueness(
        &self,
        _tx: &Self::Tx,
        _context: &C,
        _state_checkpoint: &mut StateCheckpoint<C>,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        _tx: &Self::Tx,
        _sequencer: &Da::Address,
        _state_checkpoint: &mut StateCheckpoint<C>,
    ) {
    }
}

/// Resolves the context for a transaction.
impl<C: Context, Da: DaSpec> ContextResolver<C, Da> for TestRuntime<C, Da> {
    /// The transaction type that the resolver knows how to parse.
    type Tx = Transaction<C>;
    /// Resolves the context for a transaction.
    fn resolve_context(
        &self,
        tx: &Self::Tx,
        sequencer: &Da::Address,
        height: u64,
        working_set: &mut StateCheckpoint<C>,
    ) -> C {
        let sender = tx.pub_key().to_address();
        let sequencer = self
            .sequencer_registry
            .resolve_da_address(sequencer, working_set)
            .ok_or(anyhow::anyhow!("Sequencer was no longer registered by the time of context resolution. This is a bug")).unwrap();
        C::new(sender, sequencer, height)
    }
}
