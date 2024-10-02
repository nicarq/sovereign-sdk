use std::marker::PhantomData;
use std::sync::Arc;

use sov_modules_api::capabilities::KernelSlotHooks;
use sov_modules_api::prelude::anyhow;
use sov_modules_api::rest::StorageReceiver;
use sov_modules_api::{DaSpec, Gas, Spec, StateCheckpoint, TxEffect, TxReceiptContents};
use sov_modules_stf_blueprint::Runtime;

use crate::{PartialTransaction, RollupStateProvider};

/// The default rollup state provider. Uses the kernel and a runtime to simulate transaction execution and compute the gas price.
pub struct DefaultRollupStateProvider<
    S: Spec,
    Da: DaSpec,
    K: KernelSlotHooks<S, Da>,
    RT: Runtime<S, Da>,
    Receipt: TxReceiptContents,
> {
    phantom: PhantomData<(S, Da, K, RT, Receipt)>,
}

impl<
        S: Spec,
        Da: DaSpec,
        K: KernelSlotHooks<S, Da> + Send + Sync,
        RT: Runtime<S, Da>,
        Receipt: TxReceiptContents,
    > RollupStateProvider for Arc<DefaultRollupStateProvider<S, Da, K, RT, Receipt>>
{
    type Spec = S;

    type Receipt = Receipt;

    type Error = anyhow::Error;

    fn get_latest_base_fee_per_gas(
        storage: &StorageReceiver<Self::Spec>,
    ) -> Result<<<Self::Spec as Spec>::Gas as Gas>::Price, Self::Error> {
        let storage = storage.borrow().clone();

        let mut state = StateCheckpoint::new(storage, &K::default());

        Ok(K::default().base_fee_per_gas(&mut state))
    }

    fn simulate_execution(
        _storage: &StorageReceiver<Self::Spec>,
        _transaction: PartialTransaction<Self::Spec>,
    ) -> Result<TxEffect<Receipt>, Self::Error> {
        anyhow::bail!("Not implemented")
    }
}
