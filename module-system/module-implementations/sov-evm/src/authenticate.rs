use borsh::BorshDeserialize;
use reth_primitives::TransactionSignedEcRecovered;
use sov_modules_api::runtime::capabilities::{AuthenticationError, FatalError};
use sov_modules_api::transaction::{
    AuthenticatedTransactionAndRawHash, AuthenticatedTransactionData, Credentials, PriorityFeeBips,
};
use sov_modules_api::{CredentialId, GasMeter, Spec};

use crate::conversions::RlpConversionError;
use crate::{CallMessage, RlpEvmTransaction};

/// Authenticate raw evm transaction.
pub fn authenticate<S: Spec>(
    raw_tx: &[u8],
    _stake_meter: &mut impl GasMeter<S::Gas>,
) -> Result<(AuthenticatedTransactionAndRawHash<S>, CallMessage), AuthenticationError> {
    // TODO: Charge gas for deserialization & signature check.

    let tx = RlpEvmTransaction::try_from_slice(raw_tx).map_err(|e| {
        AuthenticationError::FatalError(FatalError::DeserializationFailed(e.to_string()))
    })?;

    let tx_clone = tx.clone();

    let evm_tx_recovered: TransactionSignedEcRecovered =
        tx.try_into().map_err(|e: RlpConversionError| {
            AuthenticationError::FatalError(FatalError::SigVerificationFailed(e.to_string()))
        })?;

    let tx_hash = evm_tx_recovered.hash();
    let (signed_tx, signer) = evm_tx_recovered.to_components();

    // TODO `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/521>`: These values should be correctly set from the raw message
    let chain_id = 0;
    let max_priority_fee_bips = PriorityFeeBips::ZERO;
    let max_fee = 10_000;
    let gas_limit = None;

    let nonce = signed_tx.nonce();

    let credential_id = CredentialId(signer.into_word().into());

    let authenticated_tx = AuthenticatedTransactionData::<S> {
        credential_id,
        credentials: Credentials::new(signer),
        default_address: None,
        chain_id,
        max_priority_fee_bips,
        max_fee,
        gas_limit,
        nonce,
    };

    let tx_and_raw_hash = AuthenticatedTransactionAndRawHash {
        raw_tx_hash: tx_hash.into(),
        authenticated_tx,
    };
    let call = CallMessage { rlp: tx_clone };
    Ok((tx_and_raw_hash, call))
}
