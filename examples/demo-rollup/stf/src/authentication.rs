//! The demo-rollup supports `EVM` and `sov-module` authenticators.
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_address::{EthereumAddress, FromVmAddress};
use sov_evm::{EthereumAuthenticator, TransactionSigned};
use sov_modules_api::capabilities::{
    fatal_deserialization_error, AuthenticationError, AuthenticationOutput, AuthorizationData,
    BatchFromUnregisteredSequencer, FatalError, UnregisteredAuthenticationError,
};
use sov_modules_api::runtime::capabilities::TransactionAuthenticator;
use sov_modules_api::transaction::TransactionWithoutCall;
use sov_modules_api::{DispatchCall, FullyBakedTx, ProvableStateReader, RawTx, Spec};
use sov_state::User;

use crate::runtime::{Runtime, RuntimeCall};

impl<S: Spec> TransactionAuthenticator<S> for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    type Decodable = <Self as DispatchCall>::Decodable;

    type AuthorizationData = AuthorizationData<S>;

    type Input = Auth;

    type Signature = Auth<TransactionSigned, TransactionWithoutCall<S>>;

    #[cfg(feature = "native")]
    fn decode_serialized_tx(
        &self,
        tx: &FullyBakedTx,
    ) -> Result<(Self::Decodable, Self::Signature), FatalError> {
        let auth_variant: Auth = borsh::from_slice(&tx.data)
            .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

        match auth_variant {
            Auth::Evm(rlp_tx) => {
                let (call, tx) = sov_evm::decode_evm_tx(&rlp_tx)?;
                Ok((
                    RuntimeCall::Evm(sov_evm::CallMessage { rlp: call }),
                    Auth::Evm(tx),
                ))
            }
            Auth::Mod(raw_tx) => {
                let (call, tx) = sov_modules_api::capabilities::decode_sov_tx::<_, Self>(&raw_tx)?;
                Ok((call, Auth::Mod(tx)))
            }
        }
    }

    fn authenticate<Accessor: ProvableStateReader<User, Spec = S>>(
        &self,
        tx: &FullyBakedTx,
        pre_exec_ws: &mut Accessor,
    ) -> Result<
        AuthenticationOutput<S, Self::Decodable, Self::AuthorizationData>,
        AuthenticationError,
    > {
        let input: Auth = borsh::from_slice(&tx.data)
            .map_err(|e| fatal_deserialization_error::<_, S, _>(&tx.data, e, pre_exec_ws))?;

        match input {
            Auth::Mod(tx) => sov_modules_api::capabilities::authenticate::<_, S, Self>(
                &tx,
                &<Runtime<S> as sov_modules_stf_blueprint::Runtime<S>>::CHAIN_HASH,
                pre_exec_ws,
            ),
            Auth::Evm(tx) => {
                let (tx_and_raw_hash, auth_data, runtime_call) =
                    sov_evm::authenticate::<_, S>(&tx, pre_exec_ws)?;
                let call = RuntimeCall::Evm(runtime_call);

                Ok((tx_and_raw_hash, auth_data, call))
            }
        }
    }

    fn authenticate_unregistered<Accessor: ProvableStateReader<User, Spec = S>>(
        &self,
        batch: &BatchFromUnregisteredSequencer,
        pre_exec_ws: &mut Accessor,
    ) -> Result<
        AuthenticationOutput<S, Self::Decodable, Self::AuthorizationData>,
        UnregisteredAuthenticationError,
    > {
        let (tx_and_raw_hash, auth_data, runtime_call) =
            sov_modules_api::capabilities::authenticate::<_, S, Runtime<S>>(
                &batch.tx.data,
                &<Runtime<S> as sov_modules_stf_blueprint::Runtime<S>>::CHAIN_HASH,
                pre_exec_ws,
            )
            .map_err(|e| match e {
                AuthenticationError::FatalError(err, hash) => {
                    UnregisteredAuthenticationError::FatalError(err, hash)
                }
                AuthenticationError::OutOfGas(err) => {
                    UnregisteredAuthenticationError::OutOfGas(err)
                }
            })?;

        match &runtime_call {
            RuntimeCall::SequencerRegistry(sov_sequencer_registry::CallMessage::Register {
                ..
            }) => Ok((tx_and_raw_hash, auth_data, runtime_call)),
            _ => Err(UnregisteredAuthenticationError::FatalError(
                FatalError::Other(
                    "The runtime call included in the transaction was invalid.".to_string(),
                ),
                tx_and_raw_hash.raw_tx_hash,
            ))?,
        }
    }

    fn add_standard_auth(tx: RawTx) -> Self::Input {
        Auth::Mod(tx.data)
    }
}

/// Describes which authenticator to use to deserialize and check the signature on
/// the transaction.
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub enum Auth<Evm = Vec<u8>, Mod = Vec<u8>> {
    /// Authenticate using the `EVM` authenticator, which expects a standard EVM transaction
    /// (i.e. an rlp-encoded payload signed using secp256k1 and hashed using keccak256).
    Evm(Evm),
    /// Authenticate using the standard `sov-module` authenticator, which uses the default
    /// signature scheme and hashing algorithm defined in the rollup's [`Spec`].
    Mod(Mod),
}

impl<S: Spec> EthereumAuthenticator<S> for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    fn add_ethereum_auth(tx: RawTx) -> <Self as TransactionAuthenticator<S>>::Input {
        Auth::Evm(tx.data)
    }
}
