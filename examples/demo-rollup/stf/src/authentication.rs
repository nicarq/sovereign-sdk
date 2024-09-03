//! The demo-rollup supports `EVM` and `sov-module` authenticators.
use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_modules_api::capabilities::{
    Authenticator, AuthorizationData, UnregisteredAuthenticationError,
};
use sov_modules_api::runtime::capabilities::{AuthenticationResult, RuntimeAuthenticator};
use sov_modules_api::{
    DaSpec, DispatchCall, GasMeter, PreExecWorkingSet, RawTx, Spec, UnlimitedGasMeter,
};
use sov_sequencer_registry::SequencerStakeMeter;

use crate::runtime::{Runtime, RuntimeCall};

impl<S: Spec, Da: DaSpec> RuntimeAuthenticator<S> for Runtime<S, Da>
where
    EthereumToRollupAddressConverter: TryInto<S::Address>,
{
    type Decodable = <Self as DispatchCall>::Decodable;

    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    type AuthorizationData = AuthorizationData<S>;

    type Input = Auth;

    fn authenticate(
        &self,
        input: &Self::Input,
        pre_exec_ws: &mut PreExecWorkingSet<S, Self::SequencerStakeMeter>,
    ) -> AuthenticationResult<S, Self::Decodable, Self::AuthorizationData> {
        match input {
            Auth::Mod(tx) => ModAuth::<S, Da>::authenticate(tx, pre_exec_ws),
            Auth::Evm(tx) => EvmAuth::<S, Da>::authenticate(tx, pre_exec_ws),
        }
    }

    fn authenticate_unregistered(
        &self,
        raw_tx: &RawTx,
        pre_exec_ws: &mut PreExecWorkingSet<S, UnlimitedGasMeter<S::Gas>>,
    ) -> AuthenticationResult<
        S,
        Self::Decodable,
        Self::AuthorizationData,
        UnregisteredAuthenticationError,
    > {
        let (tx_and_raw_hash, auth_data, runtime_call) =
            sov_modules_api::capabilities::authenticate::<
                S,
                Runtime<S, Da>,
                UnlimitedGasMeter<S::Gas>,
            >(&raw_tx.data, pre_exec_ws)?;

        match &runtime_call {
            RuntimeCall::SequencerRegistry(sov_sequencer_registry::CallMessage::Register {
                ..
            }) => Ok((tx_and_raw_hash, auth_data, runtime_call)),
            _ => Err(UnregisteredAuthenticationError::RuntimeCall)?,
        }
    }

    fn encode_standard_tx(tx: Vec<u8>) -> Self::Input {
        Auth::Mod(tx)
    }
}

/// Describes which authenticator to use to deserialize and check the signature on
/// the transaction.
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub enum Auth {
    /// Authenticate using the `EVM` authenticator, which expects a standard EVM transaction
    /// (i.e. an rlp-encoded payload signed using secp256k1 and hashed using keccak256).
    Evm(Vec<u8>),
    /// Authenticate using the standard `sov-module` authenticator, which uses the default
    /// signature scheme and hashing algorithm defined in the rollup's [`Spec`].
    Mod(Vec<u8>),
}

/// Authenticator for the sov-module system.
pub struct ModAuth<S: Spec, Da: DaSpec> {
    _phantom: PhantomData<(S, Da)>,
}

impl<S: Spec, Da: DaSpec> Authenticator for ModAuth<S, Da> {
    type Spec = S;
    type DispatchCall = Runtime<S, Da>;
    type AuthorizationData = AuthorizationData<S>;

    fn authenticate<Meter: GasMeter<S::Gas>>(
        tx: &[u8],
        pre_exec_working_set: &mut PreExecWorkingSet<S, Meter>,
    ) -> AuthenticationResult<
        Self::Spec,
        <Self::DispatchCall as DispatchCall>::Decodable,
        Self::AuthorizationData,
    > {
        sov_modules_api::capabilities::authenticate::<Self::Spec, Self::DispatchCall, Meter>(
            tx,
            pre_exec_working_set,
        )
    }

    fn encode(tx: Vec<u8>) -> anyhow::Result<RawTx> {
        let data = borsh::to_vec(&Auth::Mod(tx))?;
        Ok(RawTx { data })
    }
}

/// Authenticator for the EVM.
pub struct EvmAuth<S: Spec, Da: DaSpec> {
    _phantom: PhantomData<(S, Da)>,
}

/// A converter from an Ethereum address to a rollup address.
pub struct EthereumToRollupAddressConverter(
    /// The raw bytes of the ethereum address.
    pub [u8; 20],
);

impl From<sov_evm::EvmAddress> for EthereumToRollupAddressConverter {
    fn from(address: sov_evm::EvmAddress) -> Self {
        Self(address.into())
    }
}

impl<H> TryInto<sov_modules_api::Address<H>> for EthereumToRollupAddressConverter {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<sov_modules_api::Address<H>, Self::Error> {
        anyhow::bail!("Not implemented")
    }
}

impl<S: Spec<Address = Addr>, Da: DaSpec, Addr> Authenticator for EvmAuth<S, Da>
where
    EthereumToRollupAddressConverter: TryInto<Addr>,
    Addr: Send + Sync,
{
    type Spec = S;
    type DispatchCall = Runtime<S, Da>;
    type AuthorizationData = AuthorizationData<S>;

    fn authenticate<Meter: GasMeter<S::Gas>>(
        tx: &[u8],
        stake_meter: &mut PreExecWorkingSet<S, Meter>,
    ) -> AuthenticationResult<
        Self::Spec,
        <Self::DispatchCall as DispatchCall>::Decodable,
        Self::AuthorizationData,
    > {
        let (tx_and_raw_hash, auth_data, runtime_call) = sov_evm::authenticate::<
            Self::Spec,
            Meter,
            EthereumToRollupAddressConverter,
        >(tx, stake_meter)?;
        let call = RuntimeCall::Evm(runtime_call);

        Ok((tx_and_raw_hash, auth_data, call))
    }

    fn encode(tx: Vec<u8>) -> anyhow::Result<RawTx> {
        let data = borsh::to_vec(&Auth::Evm(tx))?;
        Ok(RawTx { data })
    }
}
