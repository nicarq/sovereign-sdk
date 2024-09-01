use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::{
    AccessoryStateReaderAndWriter, BatchSequencerReceipt, Spec, StateCheckpoint, WorkingSet,
};
use sov_rollup_interface::da::DaSpec;

use crate::runtime::Runtime;

impl<S: Spec, Da: DaSpec> TxHooks for Runtime<S, Da> {
    type Spec = S;
    type TxState = WorkingSet<S>;
}

impl<S: Spec, Da: DaSpec> ApplyBatchHooks<Da> for Runtime<S, Da> {
    type Spec = S;
    type BatchResult = BatchSequencerReceipt<Da>;

    fn begin_batch_hook(
        &self,
        _sender: &Da::Address,
        _state: &mut StateCheckpoint<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn end_batch_hook(&self, _result: &Self::BatchResult, _state: &mut StateCheckpoint<S>) {}
}

impl<S: Spec, Da: DaSpec> SlotHooks for Runtime<S, Da> {
    type Spec = S;

    fn begin_slot_hook(
        &self,
        pre_state_root: <S as Spec>::VisibleHash,
        versioned_working_set: &mut sov_modules_api::VersionedStateReadWriter<StateCheckpoint<S>>,
    ) {
        self.evm
            .begin_slot_hook(pre_state_root, versioned_working_set);
    }

    fn end_slot_hook(&self, state: &mut sov_modules_api::StateCheckpoint<S>) {
        self.evm.end_slot_hook(state);
    }
}

impl<S: Spec, Da: sov_modules_api::DaSpec> FinalizeHook for Runtime<S, Da> {
    type Spec = S;

    fn finalize_hook(
        &self,
        #[allow(unused_variables)] root_hash: S::VisibleHash,
        #[allow(unused_variables)] state: &mut impl AccessoryStateReaderAndWriter,
    ) {
        #[cfg(feature = "native")]
        self.evm.finalize_hook(root_hash, state);
    }
}
