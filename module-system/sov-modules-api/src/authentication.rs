use borsh::BorshDeserialize;
use sov_modules_core::capabilities::{AuthenticationError, RawTx};
use sov_modules_core::{DispatchCall, Spec};
use sov_rollup_interface::zk::CryptoSpec;

use crate::digest::Digest;
use crate::transaction::{Transaction, TransactionAndRawHash};

/// Authenticate raw transaction.
pub fn authenticate<S: Spec, D: DispatchCall>(
    raw_tx: &RawTx,
) -> Result<(TransactionAndRawHash<S>, D::Decodable), AuthenticationError> {
    let raw_tx_hash = <S::CryptoSpec as CryptoSpec>::Hasher::digest(&raw_tx.data).into();

    let tx = Transaction::<S>::deserialize(&mut raw_tx.data.as_slice())
        .map_err(|e| AuthenticationError::SigVerificationFailed(e.to_string()))?;

    tx.verify()
        .map_err(|e| AuthenticationError::SigVerificationFailed(e.to_string()))?;

    let runtime_call = D::decode_call(tx.runtime_msg())
        .map_err(|e| AuthenticationError::MessageDecodingFailed(e.to_string(), raw_tx_hash))?;

    let tx_and_raw_hash = TransactionAndRawHash { tx, raw_tx_hash };

    Ok((tx_and_raw_hash, runtime_call))
}
