/// Provides default implementations of each requested hook for the runtime.
#[macro_export]
macro_rules! impl_runtime_hook {
    ($runtime:ty, SlotHooks) => {
        impl<S> ::sov_modules_api::hooks::SlotHooks for $runtime
        where
            S: ::sov_modules_api::Spec,
        {
            type Spec = S;

            fn begin_slot_hook(
                &self,
                _pre_state_root: &<S::Storage as sov_state::Storage>::Root,
                _state: &mut ::sov_modules_api::StateCheckpoint<S::Storage>,
            ) {
            }

            fn end_slot_hook(&self, _state: &mut ::sov_modules_api::StateCheckpoint<S::Storage>) {}
        }
    };
    ($runtime:ty, KernelSlotHooks) => {
        impl<S> ::sov_modules_api::hooks::KernelSlotHooks for $runtime
        where
            S: ::sov_modules_api::Spec,
        {
            type Spec = S;
        }
    };
    ($runtime:ty, FinalizeHook) => {
        impl<S> ::sov_modules_api::hooks::FinalizeHook for $runtime
        where
            S: ::sov_modules_api::Spec,
        {
            type Spec = S;

            fn finalize_hook(
                &self,
                _root_hash: &<S::Storage as sov_state::Storage>::Root,
                _state: &mut impl ::sov_modules_api::prelude::StateReaderAndWriter<
                    sov_state::namespaces::Accessory,
                >,
            ) {
            }
        }
    };
    ($runtime:ty, ApplyBatchHooks) => {
        impl<S> ::sov_modules_api::hooks::ApplyBatchHooks for $runtime
        where
            S: ::sov_modules_api::Spec,
        {
            type Spec = S;
            type BatchResult = ::sov_modules_api::BatchSequencerReceipt<S>;

            fn begin_batch_hook(
                &self,
                _sender: &<<S as ::sov_modules_api::Spec>::Da as ::sov_modules_api::DaSpec>::Address,
                _state_checkpoint: &mut ::sov_modules_api::TxScratchpad<S::Storage>,
            ) -> anyhow::Result<()> {
                Ok(())
            }

            fn end_batch_hook(
                &self,
                _result: &Self::BatchResult,
                _state_checkpoint: &mut ::sov_modules_api::TxScratchpad<S::Storage>,
            ) {
            }
        }
    };
    ($runtime:ty, TxHooks) => {
        impl<S> ::sov_modules_api::hooks::TxHooks for $runtime
        where
            S: ::sov_modules_api::Spec,
        {
            type Spec = S;
            type TxState = ::sov_modules_api::WorkingSet<S>;

            fn pre_dispatch_tx_hook(
                &self,
                _tx: &::sov_modules_api::transaction::AuthenticatedTransactionData<S>,
                _state: &mut Self::TxState,
            ) -> anyhow::Result<()> {
                Ok(())
            }

            fn post_dispatch_tx_hook(
                &self,
                _tx: &::sov_modules_api::transaction::AuthenticatedTransactionData<S>,
                _ctx: &::sov_modules_api::Context<S>,
                _state: &mut Self::TxState,
            ) -> anyhow::Result<()> {
                Ok(())
            }
        }
    };
}
