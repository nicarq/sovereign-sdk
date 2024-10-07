//! The demo-rollup supports `EVM` and `sov-module` authenticators.
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_evm::EthereumAuthenticator;
use sov_modules_api::capabilities::{
    AuthenticationError, AuthenticationOutput, AuthorizationData, UnregisteredAuthenticationError,
};
use sov_modules_api::runtime::capabilities::TransactionAuthenticator;
use sov_modules_api::{DaSpec, DispatchCall, PreExecWorkingSet, RawTx, Spec};

use crate::runtime::{Runtime, RuntimeCall};

impl<S: Spec, Da: DaSpec> TransactionAuthenticator<S> for Runtime<S, Da>
where
    EthereumToRollupAddressConverter: TryInto<S::Address>,
{
    type Decodable = <Self as DispatchCall>::Decodable;

    type AuthorizationData = AuthorizationData<S>;

    type Input = Auth;

    fn authenticate(
        &self,
        input: &Self::Input,
        pre_exec_ws: &mut PreExecWorkingSet<S>,
    ) -> Result<
        AuthenticationOutput<S, Self::Decodable, Self::AuthorizationData>,
        AuthenticationError,
    > {
        match input {
            Auth::Mod(tx) => {
                sov_modules_api::capabilities::authenticate::<S, Self>(tx, pre_exec_ws)
            }
            Auth::Evm(tx) => {
                let (tx_and_raw_hash, auth_data, runtime_call) =
                    sov_evm::authenticate::<S, EthereumToRollupAddressConverter>(tx, pre_exec_ws)?;
                let call = RuntimeCall::Evm(runtime_call);

                Ok((tx_and_raw_hash, auth_data, call))
            }
        }
    }

    fn authenticate_unregistered(
        &self,
        raw_tx: &Self::Input,
        pre_exec_ws: &mut PreExecWorkingSet<S>,
    ) -> Result<
        AuthenticationOutput<S, Self::Decodable, Self::AuthorizationData>,
        UnregisteredAuthenticationError,
    > {
        let contents = if let Auth::Mod(tx) = raw_tx {
            tx
        } else {
            return Err(UnregisteredAuthenticationError::InvalidAuthenticator)?;
        };
        let (tx_and_raw_hash, auth_data, runtime_call) =
            sov_modules_api::capabilities::authenticate::<S, Runtime<S, Da>>(contents, pre_exec_ws)
                .map_err(|e| match e {
                    AuthenticationError::FatalError(err) => {
                        UnregisteredAuthenticationError::FatalError(err)
                    }
                    AuthenticationError::OutOfGas(err) => {
                        UnregisteredAuthenticationError::OutOfGas(err)
                    }
                })?;

        match &runtime_call {
            RuntimeCall::SequencerRegistry(sov_sequencer_registry::CallMessage::Register {
                ..
            }) => Ok((tx_and_raw_hash, auth_data, runtime_call)),
            _ => Err(UnregisteredAuthenticationError::RuntimeCall)?,
        }
    }

    fn add_standard_auth(tx: RawTx) -> Self::Input {
        Auth::Mod(tx.data)
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

impl<S: Spec, Da: DaSpec> EthereumAuthenticator<S> for Runtime<S, Da>
where
    EthereumToRollupAddressConverter: TryInto<S::Address>,
{
    fn add_ethereum_auth(tx: RawTx) -> <Self as TransactionAuthenticator<S>>::Input {
        Auth::Evm(tx.data)
    }
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
