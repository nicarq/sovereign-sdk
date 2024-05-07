use borsh::BorshDeserialize;
use sov_modules_core::capabilities::{AuthenticationError, FatalError, RawTx};
use sov_modules_core::{DispatchCall, Spec};
use sov_rollup_interface::zk::CryptoSpec;

use crate::digest::Digest;
use crate::transaction::{AuthenticatedTransactionAndRawHash, Transaction};

/// A single rollup can support several authentication mechanisms.
/// For example, within the same rollup, some transactions can be signed with the SignatureA scheme and others with the SignatureB scheme.
/// The Authenticator trait makes it possible to abstract away the details of how the transaction should be validated.
pub trait Authenticator: Send + Sync + 'static {
    /// The rollup Spec.
    type Spec: Spec;
    /// CallMessage dispatcher.
    type DispatchCall: DispatchCall;

    /// Accepts raw tx and interprets it as a transaction, performing validation relevant to a particular authentication scheme.
    #[allow(clippy::type_complexity)]
    fn authenticate(
        raw_tx: &[u8],
    ) -> Result<
        (
            AuthenticatedTransactionAndRawHash<Self::Spec>,
            <Self::DispatchCall as DispatchCall>::Decodable,
        ),
        AuthenticationError,
    >;

    /// Encodes transaction bytes using a particular authenticator.
    fn encode(tx: Vec<u8>) -> Result<RawTx, anyhow::Error>;
}

// Authenticate raw transaction.
pub fn authenticate<S: Spec, D: DispatchCall>(
    mut raw_tx: &[u8],
) -> Result<(AuthenticatedTransactionAndRawHash<S>, D::Decodable), AuthenticationError> {
    let raw_tx_hash = <S::CryptoSpec as CryptoSpec>::Hasher::digest(raw_tx).into();

    let tx = Transaction::<S>::deserialize(&mut raw_tx).map_err(|e| {
        AuthenticationError::FatalError(FatalError::DeserializationFailed(e.to_string()))
    })?;

    tx.verify().map_err(|e| {
        AuthenticationError::FatalError(FatalError::SigVerificationFailed(e.to_string()))
    })?;

    let runtime_call = D::decode_call(tx.runtime_msg()).map_err(|e| {
        AuthenticationError::FatalError(FatalError::MessageDecodingFailed(
            e.to_string(),
            raw_tx_hash,
        ))
    })?;

    let tx_and_raw_hash = AuthenticatedTransactionAndRawHash::new(raw_tx_hash, tx.into());

    Ok((tx_and_raw_hash, runtime_call))
}
