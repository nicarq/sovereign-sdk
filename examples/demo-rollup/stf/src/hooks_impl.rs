use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, KernelSlotHooks, SlotHooks, TxHooks};
use sov_modules_api::{
    AccessoryStateReaderAndWriter, BatchSequencerReceipt, Spec, TxScratchpad, WorkingSet,
};
use sov_rollup_interface::da::DaSpec;
use sov_state::Storage;

use crate::runtime::Runtime;

impl<S: Spec> TxHooks for Runtime<S> {
    type Spec = S;
    type TxState = WorkingSet<S>;
}

impl<S: Spec> ApplyBatchHooks for Runtime<S> {
    type Spec = S;
    type BatchResult = BatchSequencerReceipt<S::Da>;

    fn begin_batch_hook(
        &self,
        _sender: &<S::Da as DaSpec>::Address,
        _state: &mut TxScratchpad<S::Storage>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn end_batch_hook(&self, _result: &Self::BatchResult, _state: &mut TxScratchpad<S::Storage>) {}
}

impl<S: Spec> SlotHooks for Runtime<S> {
    type Spec = S;

    fn begin_slot_hook(
        &self,
        pre_state_root: &<<S as Spec>::Storage as Storage>::Root,
        versioned_working_set: &mut sov_modules_api::StateCheckpoint<S::Storage>,
    ) {
        self.evm
            .begin_slot_hook(pre_state_root, versioned_working_set);
    }

    fn end_slot_hook(&self, state: &mut sov_modules_api::StateCheckpoint<S::Storage>) {
        self.evm.end_slot_hook(state);
    }
}

impl<S: Spec> KernelSlotHooks for Runtime<S> {
    type Spec = S;
}

impl<S: Spec> FinalizeHook for Runtime<S> {
    type Spec = S;

    fn finalize_hook(
        &self,
        #[allow(unused_variables)] root_hash: &<<S as Spec>::Storage as Storage>::Root,
        #[allow(unused_variables)] state: &mut impl AccessoryStateReaderAndWriter,
    ) {
        #[cfg(feature = "native")]
        self.evm.finalize_hook(root_hash, state);
    }
}
