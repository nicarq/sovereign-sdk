use std::sync::{Arc, RwLock};

use sov_chain_state::ChainStateConfig;
use sov_modules_api::hooks::{ApplyBlobHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::macros::DefaultRuntime;
use sov_modules_api::runtime::capabilities::Kernel;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{
    AccessoryWorkingSet, BlobReaderTrait, Context, DaSpec, DispatchCall, Event, GasUnit, Genesis,
    MessageCodec, PublicKey, Spec,
};
use sov_modules_stf_blueprint::kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_modules_stf_blueprint::{GenesisParams, Runtime, RuntimeTxHook, SequencerOutcome};
use sov_state::Storage;
use sov_value_setter::{ValueSetter, ValueSetterConfig};

#[derive(Genesis, DispatchCall, Event, MessageCodec, DefaultRuntime)]
#[serialization(borsh::BorshDeserialize, borsh::BorshSerialize)]
pub(crate) struct TestRuntime<C: Context> {
    pub value_setter: ValueSetter<C>,
}

pub(crate) fn create_chain_state_genesis_config<C: Context, Da: DaSpec>(
    admin_pub_key: <C as Spec>::Address,
) -> GenesisParams<GenesisConfig<C>, BasicKernelGenesisConfig<C, Da>> {
    let runtime_config: <TestRuntime<C> as sov_modules_stf_blueprint::Runtime<C, Da>>::GenesisConfig =
        GenesisConfig { value_setter: ValueSetterConfig { admin: admin_pub_key } };
    let kernel_config: <TestKernel<C, Da> as Kernel<C, Da>>::GenesisConfig =
        BasicKernelGenesisConfig {
            chain_state: ChainStateConfig {
                initial_slot_height: 0,
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

impl<C: Context> TxHooks for TestRuntime<C> {
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

impl<C: Context, B: BlobReaderTrait> ApplyBlobHooks<B> for TestRuntime<C> {
    type Context = C;
    type BlobResult = SequencerOutcome<<B as BlobReaderTrait>::Address>;

    fn begin_blob_hook(
        &self,
        _blob: &mut B,
        _working_set: &mut sov_modules_api::WorkingSet<C>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn end_blob_hook(
        &self,
        _result: Self::BlobResult,
        _working_set: &mut sov_modules_api::WorkingSet<C>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

impl<C: Context> SlotHooks for TestRuntime<C> {
    type Context = C;

    fn begin_slot_hook(
        &self,
        _pre_state_root: &<<Self::Context as Spec>::Storage as Storage>::Root,
        _working_set: &mut sov_modules_api::VersionedWorkingSet<C>,
    ) {
    }

    fn end_slot_hook(&self, _working_set: &mut sov_modules_api::WorkingSet<C>) {}
}

impl<C: Context> FinalizeHook for TestRuntime<C> {
    type Context = C;

    fn finalize_hook(
        &self,
        _root_hash: &<<Self::Context as Spec>::Storage as Storage>::Root,
        _accesorry_working_set: &mut AccessoryWorkingSet<C>,
    ) {
    }
}

impl<C: Context, Da: DaSpec> Runtime<C, Da> for TestRuntime<C> {
    type GenesisConfig = GenesisConfig<C>;

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
