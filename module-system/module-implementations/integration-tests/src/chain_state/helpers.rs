use std::sync::{Arc, RwLock};

use sov_bank::{get_genesis_token_address, Bank, BankConfig, Coins, TokenConfig};
use sov_chain_state::ChainStateConfig;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::macros::DefaultRuntime;
use sov_modules_api::runtime::capabilities::Kernel;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{
    AccessoryWorkingSet, Context, DaSpec, DispatchCall, Event, GasUnit, Genesis, MessageCodec,
    PublicKey, Spec,
};
use sov_modules_stf_blueprint::kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_modules_stf_blueprint::{GenesisParams, Runtime, RuntimeTxHook, SequencerOutcome};
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
    type PreArg = RuntimeTxHook<C>;
    type PreResult = C;

    fn pre_dispatch_tx_hook(
        &self,
        tx: &Transaction<Self::Context>,
        _working_set: &mut sov_modules_api::WorkingSet<C>,
        arg: &RuntimeTxHook<C>,
    ) -> anyhow::Result<C> {
        let RuntimeTxHook { height, sequencer } = arg;
        let sender = tx.pub_key().to_address();
        let sequencer = sequencer.to_address();

        Ok(C::new(sender, sequencer, *height))
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
        _working_set: &mut sov_modules_api::WorkingSet<C>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn end_batch_hook(
        &self,
        _result: Self::BatchResult,
        _working_set: &mut sov_modules_api::WorkingSet<C>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

impl<C: Context, Da: DaSpec> SlotHooks for TestRuntime<C, Da> {
    type Context = C;

    fn begin_slot_hook(
        &self,
        _pre_state_root: &<<Self::Context as Spec>::Storage as Storage>::Root,
        _working_set: &mut sov_modules_api::VersionedWorkingSet<C>,
    ) {
    }

    fn end_slot_hook(&self, _working_set: &mut sov_modules_api::WorkingSet<C>) {}
}

impl<C: Context, Da: DaSpec> FinalizeHook for TestRuntime<C, Da> {
    type Context = C;

    fn finalize_hook(
        &self,
        _root_hash: &<<Self::Context as Spec>::Storage as Storage>::Root,
        _accesorry_working_set: &mut AccessoryWorkingSet<C>,
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
