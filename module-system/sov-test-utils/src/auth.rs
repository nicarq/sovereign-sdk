use std::marker::PhantomData;

use sov_modules_api::runtime::capabilities::RawTx;
use sov_modules_api::transaction::AuthenticatedTransactionAndRawHash;
use sov_modules_api::{Authenticator, DaSpec, DispatchCall, GasMeter, Spec};

use crate::runtime::TestRuntime;

/// Test authenticator.
pub struct TestAuth<S: Spec, Da: DaSpec> {
    _phantom: PhantomData<(S, Da)>,
}

impl<S: Spec, Da: DaSpec> Authenticator for TestAuth<S, Da> {
    type Spec = S;
    type DispatchCall = TestRuntime<S, Da>;

    fn authenticate(
        tx: &[u8],
        stake_meter: &mut impl GasMeter<S::Gas>,
    ) -> Result<
        (
            AuthenticatedTransactionAndRawHash<Self::Spec>,
            <Self::DispatchCall as DispatchCall>::Decodable,
        ),
        sov_modules_api::runtime::capabilities::AuthenticationError,
    > {
        sov_modules_api::authenticate::<Self::Spec, Self::DispatchCall>(tx, stake_meter)
    }

    fn encode(tx: Vec<u8>) -> Result<sov_modules_api::runtime::capabilities::RawTx, anyhow::Error> {
        Ok(RawTx { data: tx })
    }
}
