mod data;
mod rewards;
use std::fmt::Debug;
use std::io;

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

use crate::{
    DispatchCall, Gas, GasMeter, GasMeteringError, GasSpec, MeteredBorshDeserialize,
    MeteredBorshDeserializeError, MeteredSigVerificationError, MeteredSignature, Spec,
};

#[cfg(test)]
mod tests;

/// Structures that implement this trait represent a call message that can be included in a
/// transaction.
/// By default, this is blanket-derived on anything implementing the `Runtime` trait, implementing
/// the trait using the runtime's typed `RuntimeCall` messages.
pub trait TransactionCallable {
    /// The type of the call of the transaction.
    type Call: BorshSerialize + BorshDeserialize + Debug + Clone + PartialEq + Eq;
}

impl<D: DispatchCall> TransactionCallable for D {
    type Call = D::Decodable;
}

/// A Transaction object that is compatible with the module-system/sov-default-stf.
#[derive(
    derive_more::Debug, // derive_more uses the correct bound of TransactionCallable::RuntimeCall
    Clone,
)]
#[cfg_attr(
    feature = "native",
    derive(
        sov_rollup_interface::sov_universal_wallet::UniversalWallet,
        serde::Serialize,
        serde::Deserialize,
        borsh::BorshSerialize,
    ),
    serde(bound = "R::Call: serde::Serialize + serde::de::DeserializeOwned")
)]
pub struct Transaction<R: TransactionCallable, S: Spec> {
    /// The signature of the transaction.
    pub signature: <S::CryptoSpec as CryptoSpec>::Signature,
    /// The public key of the sender of the transaction.
    pub pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
    /// The runtime call of the transaction.     
    pub runtime_call: R::Call,
    /// The nonce of the transaction.
    pub nonce: u64,
    /// The transaction metadata. Contains gas parameters and the chain ID.
    pub details: TxDetails<S>,
}

impl<R: TransactionCallable, S: Spec> Transaction<R, S> {
    fn unmetered_deserialize_inner(buf: &mut &[u8]) -> Result<Self, io::Error> {
        let signature =
            <<S::CryptoSpec as CryptoSpec>::Signature as BorshDeserialize>::deserialize(buf)?;
        let pub_key =
            <<S::CryptoSpec as CryptoSpec>::PublicKey as BorshDeserialize>::deserialize(buf)?;
        let runtime_call = <R::Call>::deserialize(buf)?;
        let nonce = <u64 as BorshDeserialize>::deserialize(buf)?;
        let details = <TxDetails<S> as BorshDeserialize>::deserialize(buf)?;

        let this = Self {
            signature,
            pub_key,
            runtime_call,
            nonce,
            details,
        };
        tracing::trace!(transaction = ?this, "Deserialized transaction");

        Ok(this)
    }
}

impl<R: TransactionCallable, S: Spec> MeteredBorshDeserialize<S> for Transaction<R, S> {
    fn deserialize(
        buf: &mut &[u8],
        meter: &mut impl GasMeter<<S as GasSpec>::Gas>,
    ) -> Result<Self, MeteredBorshDeserializeError<<S as GasSpec>::Gas>> {
        meter
            .charge_gas(&Self::gas_cost_to_deserialize(buf))
            .map_err(MeteredBorshDeserializeError::GasError)?;

        Transaction::<R, S>::unmetered_deserialize_inner(buf)
            .map_err(MeteredBorshDeserializeError::IOError)
    }

    #[cfg(feature = "native")]
    fn unmetered_deserialize(
        buf: &mut &[u8],
    ) -> Result<Self, MeteredBorshDeserializeError<<S as GasSpec>::Gas>> {
        Transaction::<R, S>::unmetered_deserialize_inner(buf)
            .map_err(MeteredBorshDeserializeError::IOError)
    }
}

// Unfortunately built-in Rust derives for Eq and PartialEq use a bound of `TransactionCallable:
// Eq, PartialEq` which is incorrect (TransactionCallable will be the Runtime in most cases).
// Thus we have to manually derive them, because the real bound is
// `TransactionCallable::RuntimeCall` (aka `<Runtime as DispatchCall>::Decodable`) which is already
// enforced in the trait definition.
impl<R: TransactionCallable, S: Spec> PartialEq for Transaction<R, S> {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature
            && self.pub_key == other.pub_key
            && self.runtime_call == other.runtime_call
            && self.nonce == other.nonce
            && self.details == other.details
    }
}
impl<R: TransactionCallable, S: Spec> Eq for Transaction<R, S> {}

/// a [`Transaction`] with the runtime_call removed
pub struct TransactionWithoutCall<S: Spec> {
    /// The signature of the transaction.
    pub signature: <S::CryptoSpec as CryptoSpec>::Signature,
    /// The public key of the sender of the transaction.
    pub pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
    /// The nonce of the transaction.
    pub nonce: u64,
    /// The transaction metadata. Contains gas parameters and the chain ID.
    pub details: TxDetails<S>,
}

impl<S: Spec> TransactionWithoutCall<S> {
    /// Construct a [`Transaction`] by adding back the appropriate CallMessage.
    pub fn with_call<R: TransactionCallable>(self, runtime_call: R::Call) -> Transaction<R, S> {
        let TransactionWithoutCall {
            nonce,
            details,
            signature,
            pub_key,
        } = self;
        Transaction {
            nonce,
            details,
            signature,
            pub_key,
            runtime_call,
        }
    }
}

/// Errors that can be raised by the [`Transaction::verify`] method.
#[derive(Error, Debug)]
pub enum TransactionVerificationError<GU: Gas> {
    /// An error occurred when deserializing the transaction.
    #[error("Impossible to deserialize transaction: {0}")]
    TransactionDeserializationError(String),
    /// The signature check failed.
    #[error("Invalid signature: {0}")]
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

impl<R: TransactionCallable, S: Spec> Transaction<R, S> {
    /// Returns a reference to the signature of the transaction.
    pub fn signature(&self) -> &<S::CryptoSpec as CryptoSpec>::Signature {
        &self.signature
    }

    /// Returns a reference to the public key of the sender of the transaction.
    pub fn pub_key(&self) -> &<S::CryptoSpec as CryptoSpec>::PublicKey {
        &self.pub_key
    }

    /// Returns a reference to the runtime call of the transaction.
    pub fn runtime_call(&self) -> &R::Call {
        &self.runtime_call
    }

    /// Check whether the transaction has been signed correctly.
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    pub fn verify(
        &self,
        chain_hash: &[u8; 32],
        meter: &mut impl GasMeter<S::Gas>,
    ) -> Result<(), TransactionVerificationError<S::Gas>> {
        let mut serialized_tx = borsh::to_vec(&self.to_unsigned_transaction()).map_err(|e| {
            TransactionVerificationError::TransactionDeserializationError(e.to_string())
        })?;
        serialized_tx.extend_from_slice(chain_hash);
        MeteredSignature::new::<S>(self.signature.clone())
            .verify(&self.pub_key, &serialized_tx, meter)
            .map_err(TransactionVerificationError::from)?;

        Ok(())
    }

    /// Creates a new transaction with the provided metadata.
    pub fn new_with_details(
        pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
        runtime_call: R::Call,
        signature: <S::CryptoSpec as CryptoSpec>::Signature,
        nonce: u64,
        details: TxDetails<S>,
    ) -> Self {
        Self {
            signature,
            runtime_call,
            pub_key,
            nonce,
            details,
        }
    }

    /// Extract the runtime call from the transaction
    pub fn split(self) -> (TransactionWithoutCall<S>, R::Call) {
        let Transaction {
            runtime_call,
            nonce,
            details,
            signature,
            pub_key,
        } = self;
        (
            TransactionWithoutCall {
                nonce,
                details,
                signature,
                pub_key,
            },
            runtime_call,
        )
    }

    fn to_unsigned_transaction(&self) -> UnsignedTransaction<R, S> {
        UnsignedTransaction::new_with_details(
            self.runtime_call.clone(),
            self.nonce,
            self.details.clone(),
        )
    }
}

#[cfg(feature = "native")]
impl<R: TransactionCallable, S: Spec> Transaction<R, S> {
    /// New signed transaction.
    pub fn new_signed_tx(
        priv_key: &<S::CryptoSpec as CryptoSpec>::PrivateKey,
        chain_hash: &[u8; 32],
        unsigned_tx: UnsignedTransaction<R, S>,
    ) -> Self {
        let mut utx_bytes: Vec<u8> = Vec::new();
        BorshSerialize::serialize(&unsigned_tx, &mut utx_bytes).unwrap();
        utx_bytes.extend_from_slice(chain_hash);

        let pub_key = priv_key.pub_key();
        let signature = priv_key.sign(&utx_bytes);

        unsigned_tx.to_signed_tx(pub_key, signature)
    }
}

/// An unsent transaction with the required data to be submitted to the DA layer
#[derive(derive_more::Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "native",
    derive(sov_rollup_interface::sov_universal_wallet::UniversalWallet)
)]
pub struct UnsignedTransaction<R: TransactionCallable, S: Spec> {
    // The runtime call
    runtime_call: R::Call,
    // The nonce
    nonce: u64,
    // Data related to fees and gas handling.
    details: TxDetails<S>,
}
// Manually implemented to ensure correct trait bounds for the same reason as for `Transaction`
// above
impl<R: TransactionCallable, S: Spec> PartialEq for UnsignedTransaction<R, S> {
    fn eq(&self, other: &Self) -> bool {
        self.runtime_call == other.runtime_call
            && self.nonce == other.nonce
            && self.details == other.details
    }
}
impl<R: TransactionCallable, S: Spec> Eq for UnsignedTransaction<R, S> {}

impl<R: TransactionCallable, S: Spec> UnsignedTransaction<R, S> {
    /// Creates a new [`UnsignedTransaction`] with the given arguments.
    pub const fn new(
        runtime_call: R::Call,
        chain_id: u64,
        max_priority_fee_bips: PriorityFeeBips,
        max_fee: u64,
        nonce: u64,
        gas_limit: Option<S::Gas>,
    ) -> Self {
        Self {
            runtime_call,
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
    pub const fn new_with_details(
        runtime_call: R::Call,
        nonce: u64,
        details: TxDetails<S>,
    ) -> Self {
        Self {
            runtime_call,
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
    ) -> Transaction<R, S> {
        Transaction::new_with_details(
            pub_key,
            self.runtime_call,
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
