use borsh::BorshDeserialize;
use reth_primitives::TransactionSigned;
use sov_address::EthereumAddress;
use sov_modules_api::capabilities::{
    fatal_deserialization_error, AuthenticationOutput, AuthorizationData, FatalError,
    TransactionAuthenticator,
};
use sov_modules_api::macros::config_value;
use sov_modules_api::runtime::capabilities::AuthenticationError;
use sov_modules_api::transaction::{
    AuthenticatedTransactionAndRawHash, AuthenticatedTransactionData, Credentials, PriorityFeeBips,
};
use sov_modules_api::{FullyBakedTx, ProvableStateReader, RawTx, Spec};
use sov_rollup_interface::TxHash;
use sov_state::User;

use crate::conversions::RlpConversionError;
use crate::{CallMessage, RlpEvmTransaction};

/// Authenticates a raw evm transaction.
///
/// Due to unfortunate limitations of the Rust type system, this function is generic over an `EvmToRollupAddressConverter` which
/// is required to implement `From<reth_primitives::Address>` and `TryInto<S::Address>`. If the caller wishes to support deriving
/// rollup addresses from the evm address, their implementation of `EvmToRollupAddressConverter` should always return Some(S::Address).
/// Otherwise, they should simply return None.
///
/// # Security
///
/// If the caller does plan to derive rollup addresses from evm addresses, they should be sure that their scheme for doing so is deterministic and
/// collision resistant. You don't want someone to be able to pick a rollup address that someone else is already using!
pub fn authenticate<Accessor: ProvableStateReader<User, Spec = S>, S: Spec>(
    raw_tx: &[u8],
    state: &mut Accessor,
) -> Result<AuthenticationOutput<S, CallMessage, AuthorizationData<S>>, AuthenticationError>
where
    S::Address: From<EthereumAddress>,
{
    // TODO: Charge gas for deserialization & signature check.

    let (rlp, tx) = parse_input(raw_tx)
        .map_err(|e| fatal_deserialization_error::<Accessor, S, _>(raw_tx, e, state))?;
    let hash = TxHash::new(tx.hash().into());
    let signer = tx.recover_signer().ok_or(AuthenticationError::FatalError(
        FatalError::SigVerificationFailed(format!("Invalid ethereum signature: tx hash {}", hash)),
        hash,
    ))?;

    let chain_id = config_value!("CHAIN_ID");
    let max_priority_fee_bips = PriorityFeeBips::ZERO;
    let max_fee = 10_000_000;
    let gas_limit = None;

    let nonce = tx.nonce();

    let credentials = Credentials::new(signer);
    let credential_id = signer.into_word().0.into();

    let authenticated_tx = AuthenticatedTransactionData::<S> {
        chain_id,
        max_priority_fee_bips,
        max_fee,
        gas_limit,
    };

    let tx_and_raw_hash = AuthenticatedTransactionAndRawHash {
        raw_tx_hash: hash,
        authenticated_tx,
    };

    let ethereum_address: EthereumAddress = signer.into();
    let auth_data = AuthorizationData {
        nonce,
        credential_id,
        credentials,
        default_address: Some(ethereum_address.into()),
    };
    let call = CallMessage { rlp };
    Ok((tx_and_raw_hash, auth_data, call))
}

/// Decode a byte sequence into an EVM transaction without checking the signature
pub fn parse_input(raw_tx: &[u8]) -> Result<(RlpEvmTransaction, TransactionSigned), FatalError> {
    let tx_data = RlpEvmTransaction::try_from_slice(raw_tx)
        .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

    if tx_data.rlp.is_empty() {
        return Err(FatalError::DeserializationFailed(
            RlpConversionError::EmptyRawTx.to_string(),
        ));
    }

    let tx = TransactionSigned::decode_enveloped(&mut &tx_data.rlp[..])
        .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

    Ok((tx_data, tx))
}

/// Indicates that a runtime supports the `Ethereum` transaction authenticator
/// and provides suitable methods for encoding and decoding Ethereum transactions.
pub trait EthereumAuthenticator<S: Spec>: TransactionAuthenticator<S> {
    /// Add the Ethereum discriminant to a transaction the runtime.
    fn add_ethereum_auth(tx: RawTx) -> <Self as TransactionAuthenticator<S>>::Input;

    /// Encode a transaction with the Ethereum discriminant for the runtime.
    fn encode_with_ethereum_auth(tx: RawTx) -> FullyBakedTx {
        <Self as TransactionAuthenticator<S>>::encode_athenticator_input(&Self::add_ethereum_auth(
            tx,
        ))
    }
}
