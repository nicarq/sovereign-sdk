mod data;
mod rewards;

use borsh::{BorshDeserialize, BorshSerialize};
pub use data::{AuthenticatedTransactionData, Credentials, PriorityFeeBips, TxDetails};
pub(crate) use rewards::transaction_consumption_helper;
pub use rewards::{ProverRewards, RemainingFunds, SequencerReward, TransactionConsumption};
use serde::{Deserialize, Serialize};
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
#[cfg(feature = "native")]
pub use sov_rollup_interface::crypto::PrivateKey;
use sov_rollup_interface::crypto::SigVerificationError;
use sov_rollup_interface::zk::CryptoSpec;
use sov_rollup_interface::TxHash;
use thiserror::Error;

use crate::{Gas, GasMeter, GasMeteringError, MeteredSigVerificationError, MeteredSignature, Spec};

#[cfg(test)]
mod tests;

/// A Transaction object that is compatible with the module-system/sov-default-stf.
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Transaction<S: Spec> {
    /// The signature of the transaction.
    pub signature: <S::CryptoSpec as CryptoSpec>::Signature,
    /// The public key of the sender of the transaction.
    pub pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
    /// The runtime message of the transaction. The message should have been encoded using the [`crate::EncodeCall`] trait.
    pub runtime_msg: Vec<u8>,
    /// The nonce of the transaction.
    pub nonce: u64,
    /// The transaction metadata. Contains gas parameters and the chain ID.
    pub details: TxDetails<S>,
}

/// Errors that can be raised by the [`Transaction::verify`] method.
#[derive(Error, Debug)]
pub enum TransactionVerificationError<GU: Gas> {
    /// An error occurred when deserializing the transaction.
    #[error("Impossible to deserialize transaction: {0}")]
    TransactionDeserializationError(String),
    /// The signature check failed.
    #[error("Signature verification error: {0}")]
    BadSignature(SigVerificationError),
    /// There is not enough gas to verify the signature.
    #[error("A gas error was raised when trying to verify the signature, {0}")]
    GasError(GasMeteringError<GU>),
}

impl<GU: Gas> From<MeteredSigVerificationError<GU>> for TransactionVerificationError<GU> {
    fn from(value: MeteredSigVerificationError<GU>) -> TransactionVerificationError<GU> {
        match value {
            MeteredSigVerificationError::BadSignature(err) => {
                TransactionVerificationError::BadSignature(err)
            }
            MeteredSigVerificationError::GasError(err) => {
                TransactionVerificationError::GasError(err)
            }
        }
    }
}

impl<S: Spec> Transaction<S> {
    /// Returns a reference to the signature of the transaction.
    pub fn signature(&self) -> &<S::CryptoSpec as CryptoSpec>::Signature {
        &self.signature
    }

    /// Returns a reference to the public key of the sender of the transaction.
    pub fn pub_key(&self) -> &<S::CryptoSpec as CryptoSpec>::PublicKey {
        &self.pub_key
    }

    /// Returns a reference to the runtime message of the transaction.
    pub fn runtime_msg(&self) -> &[u8] {
        &self.runtime_msg
    }

    /// Check whether the transaction has been signed correctly.
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    pub fn verify(
        &self,
        meter: &mut impl GasMeter<S::Gas>,
    ) -> Result<(), TransactionVerificationError<S::Gas>> {
        let serialized_tx = borsh::to_vec(&self.to_unsigned_transaction()).map_err(|e| {
            TransactionVerificationError::TransactionDeserializationError(e.to_string())
        })?;
        MeteredSignature::new::<S>(self.signature.clone())
            .verify(&self.pub_key, &serialized_tx, meter)
            .map_err(TransactionVerificationError::from)?;

        Ok(())
    }

    /// Creates a new transaction with the provided metadata.
    pub fn new_with_details(
        pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
        message: Vec<u8>,
        signature: <S::CryptoSpec as CryptoSpec>::Signature,
        nonce: u64,
        details: TxDetails<S>,
    ) -> Self {
        Self {
            signature,
            runtime_msg: message,
            pub_key,
            nonce,
            details,
        }
    }

    fn to_unsigned_transaction(&self) -> UnsignedTransaction<S> {
        UnsignedTransaction::new_with_details(
            self.runtime_msg.clone(),
            self.nonce,
            self.details.clone(),
        )
    }
}

#[cfg(feature = "native")]
impl<S: Spec> Transaction<S> {
    /// New signed transaction.
    pub fn new_signed_tx(
        priv_key: &<S::CryptoSpec as CryptoSpec>::PrivateKey,
        unsigned_tx: UnsignedTransaction<S>,
    ) -> Self {
        let mut utx_bytes: Vec<u8> = Vec::new();
        BorshSerialize::serialize(&unsigned_tx, &mut utx_bytes).unwrap();

        let pub_key = priv_key.pub_key();
        let signature = priv_key.sign(&utx_bytes);

        unsigned_tx.to_signed_tx(pub_key, signature)
    }
}

/// An unsent transaction with the required data to be submitted to the DA layer
#[derive(Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct UnsignedTransaction<S: Spec> {
    // The runtime message
    runtime_msg: Vec<u8>,
    // The nonce
    nonce: u64,
    // Data related to fees and gas handling.
    details: TxDetails<S>,
}

impl<S: Spec> UnsignedTransaction<S> {
    /// Creates a new [`UnsignedTransaction`] with the given arguments.
    pub const fn new(
        runtime_msg: Vec<u8>,
        chain_id: u64,
        max_priority_fee_bips: PriorityFeeBips,
        max_fee: u64,
        nonce: u64,
        gas_limit: Option<S::Gas>,
    ) -> Self {
        Self {
            runtime_msg,
            nonce,
            details: TxDetails {
                max_priority_fee_bips,
                max_fee,
                gas_limit,
                chain_id,
            },
        }
    }

    /// Creates a new unsigned transaction with the provided metadata.
    pub const fn new_with_details(runtime_msg: Vec<u8>, nonce: u64, details: TxDetails<S>) -> Self {
        Self {
            runtime_msg,
            nonce,
            details,
        }
    }

    /// Creates a new [`Transaction`] from this [`UnsignedTransaction`] when given a signature
    /// and a public key.
    pub fn to_signed_tx(
        self,
        pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
        signature: <S::CryptoSpec as CryptoSpec>::Signature,
    ) -> Transaction<S> {
        Transaction::new_with_details(
            pub_key,
            self.runtime_msg,
            signature,
            self.nonce,
            self.details,
        )
    }
}

/// A struct containing an authenticated transaction and its associated hash.
pub struct AuthenticatedTransactionAndRawHash<S: Spec> {
    /// Hash of raw bytes.
    pub raw_tx_hash: TxHash,
    /// Authenticated transaction data.
    pub authenticated_tx: AuthenticatedTransactionData<S>,
}
