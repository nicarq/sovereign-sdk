use std::marker::PhantomData;
use std::sync::Arc;

use sov_modules_api::capabilities::{
    AuthorizationData, HasCapabilities, KernelSlotHooks, TransactionAuthorizer,
};
use sov_modules_api::prelude::anyhow;
use sov_modules_api::rest::StorageReceiver;
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{
    BasicGasMeter, DaSpec, ExecutionContext, Gas, Spec, StateCheckpoint, VersionReader, WorkingSet,
};
use sov_modules_stf_blueprint::{apply_tx, ApplyTxResult, Runtime};
use sov_rollup_interface::common::HexString;

use crate::{PartialTransaction, RollupStateProvider};

/// The default rollup state provider. Uses the kernel and a runtime to simulate transaction execution and compute the gas price.
pub struct DefaultRollupStateProvider<S: Spec, K: KernelSlotHooks<S>, RT: Runtime<S>> {
    phantom: PhantomData<(S, K, RT)>,
}

impl<S: Spec, K: KernelSlotHooks<S> + Send + Sync, RT: Runtime<S>> RollupStateProvider
    for Arc<DefaultRollupStateProvider<S, K, RT>>
where
    RT: HasCapabilities<S, AuthorizationData = AuthorizationData<S>>,
{
    type Spec = S;

    type Runtime = RT;

    type Error = anyhow::Error;

    fn get_latest_base_fee_per_gas(
        storage: &StorageReceiver<Self::Spec>,
    ) -> Result<<<Self::Spec as Spec>::Gas as Gas>::Price, Self::Error> {
        let storage = storage.borrow().clone();

        let mut state = StateCheckpoint::new(storage, &K::default());

        Ok(K::default().base_fee_per_gas(&mut state))
    }

    fn simulate_execution(
        storage: &StorageReceiver<Self::Spec>,
        default_sequencer: <<Self::Spec as Spec>::Da as DaSpec>::Address,
        transaction: PartialTransaction<Self::Spec>,
    ) -> Result<ApplyTxResult<S>, Self::Error> {
        let auth_data =
            <<RT as HasCapabilities<S>>::AuthorizationData>::from(
                <PartialTransaction<S> as Into<AuthorizationData<S>>>::into(transaction.clone()),
            );

        let mut state = StateCheckpoint::new(storage.borrow().clone(), &K::default());

        let height = state.rollup_height_to_access();

        let gas_price = transaction
            .gas_price
            .unwrap_or_else(|| K::default().base_fee_per_gas(&mut state));

        let sequencer_da_address = transaction.sequencer.unwrap_or(default_sequencer);

        let runtime = RT::default();

        let tx_data = AuthenticatedTransactionData {
            chain_id: transaction.details.chain_id,
            max_priority_fee_bips: transaction.details.max_priority_fee_bips,
            max_fee: transaction.details.max_fee,
            gas_limit: transaction.details.gas_limit,
        };

        let mut scratchpad = state.to_tx_scratchpad();

        let decoded_call_message = RT::decode_call(
            &transaction.encoded_call_message,
            &mut BasicGasMeter::new(u64::MAX, gas_price.clone()),
        )
        .map_err(|e| anyhow::anyhow!("Unable to deserialize call message: {e}"))?;

        let ctx = runtime.transaction_authorizer().resolve_context(
            &auth_data,
            &sequencer_da_address,
            height,
            &mut scratchpad,
            // TODO(@theochap): maybe we should let the node set this variable?
            ExecutionContext::Node,
        )?;

        let working_set = WorkingSet::<S>::create_working_set(
            scratchpad,
            // We are using a fresh gas meter here to not include the costs of the pre-execution checks.
            // We may want to change this in the future.
            &gas_price.clone(),
            &tx_data,
        );

        let (result, _) = apply_tx(
            &runtime,
            &ctx,
            &tx_data,
            // We don't have a way to get the raw transaction hash because it depends on the signature.
            HexString::new([0; 32]),
            decoded_call_message,
            working_set,
        );

        Ok(result)
    }
}
