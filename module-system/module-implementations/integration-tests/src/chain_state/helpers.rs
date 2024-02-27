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
    AccessoryStateCheckpoint, Context, DaSpec, DispatchCall, Event, Gas, GasArray, Genesis,
    MessageCodec, PublicKey, Spec, StateCheckpoint, WorkingSet,
};
use sov_modules_stf_blueprint::kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_modules_stf_blueprint::{GenesisParams, Runtime, SequencerOutcome};
use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};
use sov_value_setter::{ValueSetter, ValueSetterConfig};

#[derive(Genesis, DispatchCall, Event, MessageCodec, DefaultRuntime)]
#[serialization(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize
)]
pub(crate) struct TestRuntime<S: Spec, Da: DaSpec> {
    pub value_setter: ValueSetter<S>,
    pub sequencer_registry: SequencerRegistry<S, Da>,
    pub bank: Bank<S>,
}

pub(crate) fn create_chain_state_genesis_config<S: Spec, Da: DaSpec>(
    admin_pub_key: S::Address,
    seq_rollup_address: S::Address,
    seq_da_address: Da::Address,
    seq_stake_amount: u64,
    token_name: String,
    salt: u64,
    init_balance: u64,
) -> GenesisParams<GenesisConfig<S, Da>, BasicKernelGenesisConfig<S, Da>> {
    let runtime_config: <TestRuntime<S, Da> as sov_modules_stf_blueprint::Runtime<S, Da>>::GenesisConfig =
        GenesisConfig { value_setter: ValueSetterConfig { admin: admin_pub_key }, sequencer_registry: SequencerConfig{
            seq_rollup_address: seq_rollup_address.clone(),
            seq_da_address,
            coins_to_lock: Coins { amount: seq_stake_amount, token_address: get_genesis_token_address::<S>(&token_name, salt) },
            is_preferred_sequencer: true,
        }, bank: BankConfig{
            tokens: vec![TokenConfig{token_name,
            address_and_balances: vec![(seq_rollup_address.clone(), init_balance)], authorized_minters: vec![seq_rollup_address.clone()], salt}]
        } };

    let kernel_config: <TestKernel<S, Da> as Kernel<S, Da>>::GenesisConfig =
        BasicKernelGenesisConfig {
            chain_state: ChainStateConfig {
                current_time: Default::default(),
                gas_price_blocks_depth: 10,
                gas_price_maximum_elasticity: 1,
                initial_gas_price: <<S::Gas as Gas>::Price as GasArray>::ZEROED,
                minimum_gas_price: <<S::Gas as Gas>::Price as GasArray>::ZEROED,
            },
        };
    GenesisParams {
        runtime: runtime_config,
        kernel: kernel_config,
    }
}

pub(crate) type TestKernel<S, Da> = BasicKernel<S, Da>;

impl<S: Spec, Da: DaSpec> TxHooks for TestRuntime<S, Da> {
    type Spec = S;

    fn pre_dispatch_tx_hook(
        &self,
        _tx: &Transaction<Self::Spec>,
        _working_set: &mut sov_modules_api::WorkingSet<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn post_dispatch_tx_hook(
        &self,
        _tx: &Transaction<Self::Spec>,
        _ctx: &Context<S>,
        _working_set: &mut sov_modules_api::WorkingSet<S>,
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
        _state_checkpoint: &mut sov_modules_api::StateCheckpoint<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn end_batch_hook(
        &self,
        _result: Self::BatchResult,
        _working_set: &mut sov_modules_api::StateCheckpoint<S>,
    ) {
    }
}

impl<S: Spec, Da: DaSpec> SlotHooks for TestRuntime<S, Da> {
    type Spec = S;

    fn begin_slot_hook(
        &self,
        _pre_state_root: S::VisibleHash,
        _working_set: &mut sov_modules_api::VersionedStateReadWriter<StateCheckpoint<S>>,
    ) {
    }

    fn end_slot_hook(&self, _working_set: &mut sov_modules_api::StateCheckpoint<S>) {}
}

impl<S: Spec, Da: DaSpec> FinalizeHook for TestRuntime<S, Da> {
    type Spec = S;

    fn finalize_hook(
        &self,
        _root_hash: S::VisibleHash,
        _accesorry_working_set: &mut AccessoryStateCheckpoint<S>,
    ) {
    }
}

impl<S: Spec, Da: DaSpec> Runtime<S, Da> for TestRuntime<S, Da> {
    type GenesisConfig = GenesisConfig<S, Da>;

    type GenesisPaths = ();

    fn rpc_methods(_storage: Arc<RwLock<S::Storage>>) -> jsonrpsee::RpcModule<()> {
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
            .ok_or(anyhow::anyhow!("Sequencer was no longer registered by the time of context resolution. This is a bug")).unwrap();
        Context::<S>::new(sender, sequencer, height)
    }
}
