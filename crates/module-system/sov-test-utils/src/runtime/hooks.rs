/// Provides default implementations of each requested hook for the runtime.
#[macro_export]
macro_rules! impl_runtime_hook {
    ($runtime:ty, SlotHooks) => {
        impl<S, Da> ::sov_modules_api::hooks::SlotHooks for $runtime
        where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
        {
            type Spec = S;

            fn begin_slot_hook(
                &self,
                _pre_state_root: S::VisibleHash,
                _state: &mut ::sov_modules_api::StateCheckpoint<S>,
            ) {
            }

            fn end_slot_hook(&self, _state: &mut ::sov_modules_api::StateCheckpoint<S>) {}
        }
    };
    ($runtime:ty, FinalizeHook) => {
        impl<S, Da> ::sov_modules_api::hooks::FinalizeHook for $runtime
        where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
        {
            type Spec = S;

            fn finalize_hook(
                &self,
                _root_hash: S::VisibleHash,
                _state: &mut impl ::sov_modules_api::prelude::StateReaderAndWriter<
                    sov_state::namespaces::Accessory,
                >,
            ) {
            }
        }
    };
    ($runtime:ty, ApplyBatchHooks) => {
        impl<S, Da> ::sov_modules_api::hooks::ApplyBatchHooks<Da> for $runtime
        where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
        {
            type Spec = S;
            type BatchResult = ::sov_modules_api::BatchSequencerReceipt<Da>;

            fn begin_batch_hook(
                &self,
                _sender: &Da::Address,
                _state_checkpoint: &mut ::sov_modules_api::StateCheckpoint<S>,
            ) -> anyhow::Result<()> {
                Ok(())
            }

            fn end_batch_hook(
                &self,
                _result: &Self::BatchResult,
                _state_checkpoint: &mut ::sov_modules_api::StateCheckpoint<S>,
            ) {
            }
        }
    };
    ($runtime:ty, TxHooks) => {
        impl<S, Da> ::sov_modules_api::hooks::TxHooks for $runtime
        where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
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
