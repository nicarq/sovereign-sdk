//! The demo-rollup supports `EVM` and `sov-module` authenticators.
use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_modules_api::runtime::capabilities::{
    AuthenticationError, FatalError, RawTx, RuntimeAuthenticator,
};
use sov_modules_api::transaction::AuthenticatedTransactionAndRawHash;
use sov_modules_api::{Authenticator, DaSpec, DispatchCall, GasMeter, Spec};
use sov_sequencer_registry::SequencerStakeMeter;

use crate::runtime::Runtime;

impl<S: Spec, Da: DaSpec> RuntimeAuthenticator for Runtime<S, Da> {
    type Decodable = <Self as DispatchCall>::Decodable;

    type Tx = AuthenticatedTransactionAndRawHash<S>;

    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    type Gas = S::Gas;

    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn authenticate(
        &self,
        raw_tx: &RawTx,
        sequencer_stake_meter: &mut Self::SequencerStakeMeter,
    ) -> Result<(Self::Tx, Self::Decodable), AuthenticationError> {
        let auth = Auth::try_from_slice(raw_tx.data.as_slice()).map_err(|e| {
            AuthenticationError::FatalError(FatalError::DeserializationFailed(e.to_string()))
        })?;

        match auth {
            Auth::Mod(tx) => ModAuth::<S, Da>::authenticate(&tx, sequencer_stake_meter),
            Auth::Evm(tx) => EvmAuth::<S, Da>::authenticate(&tx, sequencer_stake_meter),
        }
    }
}

#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
enum Auth {
    Evm(Vec<u8>),
    Mod(Vec<u8>),
}

/// Authenticator for the sov-module system.
pub struct ModAuth<S: Spec, Da: DaSpec> {
    _phantom: PhantomData<(S, Da)>,
}

impl<S: Spec, Da: DaSpec> Authenticator for ModAuth<S, Da> {
    type Spec = S;
    type DispatchCall = Runtime<S, Da>;
    fn authenticate(
        tx: &[u8],
        stake_meter: &mut impl GasMeter<S::Gas>,
    ) -> Result<
        (
            AuthenticatedTransactionAndRawHash<Self::Spec>,
            <Self::DispatchCall as DispatchCall>::Decodable,
        ),
        AuthenticationError,
    > {
        sov_modules_api::authenticate::<Self::Spec, Self::DispatchCall>(tx, stake_meter)
    }

    fn encode(tx: Vec<u8>) -> Result<RawTx, anyhow::Error> {
        let data = Auth::Mod(tx).try_to_vec()?;
        Ok(RawTx { data })
    }
}

/// Authenticator for the EVM.
pub struct EvmAuth<S: Spec, Da: DaSpec> {
    _phantom: PhantomData<(S, Da)>,
}

impl<S: Spec, Da: DaSpec> Authenticator for EvmAuth<S, Da> {
    type Spec = S;
    type DispatchCall = Runtime<S, Da>;
    fn authenticate(
        tx: &[u8],
        stake_meter: &mut impl GasMeter<S::Gas>,
    ) -> Result<
        (
            AuthenticatedTransactionAndRawHash<Self::Spec>,
            <Self::DispatchCall as DispatchCall>::Decodable,
        ),
        AuthenticationError,
    > {
        sov_modules_api::authenticate::<Self::Spec, Self::DispatchCall>(tx, stake_meter)
    }

    fn encode(tx: Vec<u8>) -> Result<RawTx, anyhow::Error> {
        let data = Auth::Evm(tx).try_to_vec()?;
        Ok(RawTx { data })
    }
}
