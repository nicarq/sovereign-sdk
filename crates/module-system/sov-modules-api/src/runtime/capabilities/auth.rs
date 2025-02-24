//! This module defines abstractions and workflows around authenticating and authorizing
//! transactions within a rollup.
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_modules_macros::config_value_private;
use sov_rollup_interface::crypto::CredentialId;
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::TxHash;
use sov_state::User;
use thiserror::Error;

use crate::transaction::{
    AuthenticatedTransactionAndRawHash, Credentials, Transaction, TransactionVerificationError,
};
use crate::{
    metered_credential, Context, CryptoSpec, DispatchCall, FullyBakedTx, GasMeter,
    GasMeteringError, MeteredBorshDeserialize, MeteredBorshDeserializeError, MeteredHasher,
    ProvableStateReader, RawTx, Spec, StateAccessor,
};

/// The chain ID of the rollup.
pub fn config_chain_id() -> u64 {
    config_value_private!("CHAIN_ID")
}

/// A batch sent by an unregistered sequencer contains only one transaction.
pub struct BatchFromUnregisteredSequencer {
    /// The transaction.
    pub tx: RawTx,
    /// Id of the batch.
    pub id: [u8; 32],
}

/// Authenticates raw transactions, ensuring that the *claimed* sender really did sign off on the transaction. Note that
/// simply *authenticating* a transaction does not guarantee that it will actually be executed. That decision is
/// made by the [`TransactionAuthorizer`]
///
/// Implementations of this trait should provide a way to interpret the raw bytes of the transaction and authenticate it.
/// Typically, the authentication will require checking the signature of the transaction.
pub trait TransactionAuthenticator<S: Spec> {
    /// The "message" that is extracted from the transaction and passed to the runtime for execution.
    type Decodable;

    /// The input to the authenticator
    type Input: BorshDeserialize + BorshSerialize + Clone + std::fmt::Debug + Send + Sync + 'static;

    /// The signature of the transaction.
    type Signature;

    /// Authenticates a transaction (typically by checking the signature) and deserializes its contents
    /// into an executable message.
    fn authenticate<Accessor: ProvableStateReader<User, Spec = S>>(
        &self,
        tx: &FullyBakedTx,
        state: &mut Accessor,
    ) -> Result<AuthenticationOutput<S, Self::Decodable>, AuthenticationError>;

    #[cfg(feature = "native")]
    /// Decode a transaction into a message and signature.
    /// This method doesn’t charge gas for deserialization, so it’s meant for off-chain code only (hence to the `native` feature).
    fn decode_serialized_tx(
        &self,
        tx: &FullyBakedTx,
    ) -> Result<(Self::Decodable, Self::Signature), FatalError>;

    /// Authenticates raw transaction that is submitted from unregistered sequencers for the
    /// purpose of forced registration (circumventing censorship by currently registered sequencers).
    ///
    /// Implementers of this method should take care to ensure that the gas consumption of this method is low because
    /// (if authentication fails) no one pays for the gas consumed by the authentication check.
    ///
    /// This is *not*  a significant DOS vector as long as gas consumption *during authentication* is reasonably low because (1)
    /// the blob storage capability bounds the number of unregistered blobs that can be submitted,
    /// and (2) if authentication succeeds then the gas for the blob is paid by the submitter.
    fn authenticate_unregistered<Accessor: ProvableStateReader<User, Spec = S>>(
        &self,
        batch: &BatchFromUnregisteredSequencer,
        state: &mut Accessor,
    ) -> Result<AuthenticationOutput<S, Self::Decodable>, UnregisteredAuthenticationError>;

    /// Encode a standard transaction for the rollup with information describing how to authenticate it.
    fn add_standard_auth(tx: RawTx) -> Self::Input;

    /// Encode the input for the authenticator into a byte array.
    fn encode_authenticator_input(input: &Self::Input) -> FullyBakedTx {
        FullyBakedTx::new(borsh::to_vec(&input).unwrap())
    }

    /// Encode a standard transaction for the rollup with information describing how to authenticate it.
    fn encode_with_standard_auth(tx: RawTx) -> FullyBakedTx {
        Self::encode_authenticator_input(&Self::add_standard_auth(tx))
    }
}

/// Authorizes transactions to be executed.
pub trait TransactionAuthorizer<S: Spec> {
    /// Resolves the [`Context`] for a transaction.
    fn resolve_context(
        &self,
        auth_data: &AuthorizationData<S>,
        sequencer: &<<S as Spec>::Da as DaSpec>::Address,
        sequencer_rollup_address: S::Address,
        state: &mut impl StateAccessor,
    ) -> anyhow::Result<Context<S>>;

    /// Resolves the context for an unregistered transaction.
    fn resolve_unregistered_context(
        &self,
        auth_data: &AuthorizationData<S>,
        sequencer: &<<S as Spec>::Da as DaSpec>::Address,
        state: &mut impl StateAccessor,
    ) -> anyhow::Result<Context<S>>;

    /// Prevents duplicate transactions from running.
    fn check_uniqueness(
        &self,
        auth_data: &AuthorizationData<S>,
        context: &Context<S>,
        state: &mut impl StateAccessor,
    ) -> anyhow::Result<()>;

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        auth_data: &AuthorizationData<S>,
        sequencer: &<<S as Spec>::Da as DaSpec>::Address,
        state: &mut impl StateAccessor,
    ) -> anyhow::Result<()>;
}

/// Output of the authentication.
pub type AuthenticationOutput<S, Decodable> = (
    AuthenticatedTransactionAndRawHash<S>,
    AuthorizationData<S>,
    Decodable,
);

/// Error variants that can be raised as a [`AuthenticationError::FatalError`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum FatalError {
    /// Transaction deserialization failed.
    #[error("Transaction deserialization error: {0}")]
    DeserializationFailed(String),
    /// Signature verification failed.
    #[error("Signature verification failed: {0}")]
    SigVerificationFailed(String),
    /// The ChainID was invalid
    #[error("Invalid chain id: expected {expected}, got {got}")]
    InvalidChainId {
        /// The expected chain id
        expected: u64,
        /// The actual chain id
        got: u64,
    },
    /// Transaction decoding failed.
    #[error("Transaction decoding error: {0}")]
    MessageDecodingFailed(String),
    /// A variant to capture any other fatal error.
    #[error("Other: {0}")]
    Other(String),
}

/// Authentication error type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationError {
    /// The transaction authentication failed in a way that should have been detected by the sequencer before they accepted the transaction. The sequencer is slashed.
    #[error("Transaction authentication raised a fatal error, error: {0}, tx_hash {1}")]
    FatalError(FatalError, TxHash),
    /// The transaction authentication returned an error, but including it could have been an honest mistake. The sequencer should be charged enough to cover the cost of checking the transaction but not slashed.
    #[error("Transaction authentication ran out of gas: {0}.")]
    OutOfGas(
        /// The reason for the penalization.       
        String,
    ),
}

/// Authentication error relating to transactions submitted by an unregistered sequencer for the
/// purpose of direct sequencer registration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum UnregisteredAuthenticationError {
    /// The transaction authentication failed in a way that is unrecoverable.
    #[error("Transaction authentication raised a fatal error, error: {0}")]
    FatalError(FatalError, TxHash),
    /// Transaction run out of gas
    #[error("Transaction ran out of gas, error: {0}")]
    OutOfGas(String),
}

/// The different types of data that can be used to verify transaction uniqueness
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UniquenessData {
    /// Nonce-based uniqueness: an account's transactions must have a unique and consecutive nonces
    Nonce(u64),
    /// Generation-based uniqueness: the last PAST_TRANSACTION_GENERATION generations are cached.
    /// Transactions older than this buffer are invalid, transactions falling within it or with a
    /// higher generation are valid but must have a unique hash within their generation
    Generation(u64),
}

/// Data required to authorize a sov-transaction.
pub struct AuthorizationData<S: Spec> {
    /// The nonce of the transaction.
    pub uniqueness: UniquenessData,

    /// The hash of the transaction.
    pub tx_hash: TxHash,

    /// Credential identifier used to retrieve relevant rollup address.
    pub credential_id: CredentialId,

    /// Holds the original credentials to authenticate the transaction and
    /// provides information about which `Authenticator` was used to authenticate the transaction.
    pub credentials: Credentials,

    /// The default address.
    pub default_address: S::Address,
}

fn verify_and_decode_tx<S: Spec, D: DispatchCall<Spec = S>>(
    raw_tx_hash: TxHash,
    tx: Transaction<D, S>,
    chain_hash: &[u8; 32],
    meter: &mut impl GasMeter<Spec = S>,
) -> Result<AuthenticationOutput<S, D::Decodable>, AuthenticationError> {
    if tx.details.chain_id != config_chain_id() {
        return Err(AuthenticationError::FatalError(
            FatalError::InvalidChainId {
                expected: config_chain_id(),
                got: tx.details.chain_id,
            },
            raw_tx_hash,
        ));
    }

    tx.verify(chain_hash, meter).map_err(|e| match e {
        TransactionVerificationError::BadSignature(_)
        | TransactionVerificationError::TransactionDeserializationError(_) => {
            AuthenticationError::FatalError(
                FatalError::SigVerificationFailed(e.to_string()),
                raw_tx_hash,
            )
        }
        TransactionVerificationError::GasError(_) => AuthenticationError::OutOfGas(e.to_string()),
    })?;

    let runtime_call = tx.runtime_call().clone();
    let pub_key = tx.pub_key().clone();
    let credential_id = metered_credential(&pub_key, meter)
        .map_err(|e| AuthenticationError::OutOfGas(e.to_string()))?;

    let default_address = credential_id.into();

    let credentials = Credentials::new(pub_key);
    let generation = tx.generation;

    let tx_and_raw_hash = AuthenticatedTransactionAndRawHash {
        raw_tx_hash,
        authenticated_tx: tx.into(),
    };

    Ok((
        tx_and_raw_hash,
        AuthorizationData {
            uniqueness: UniquenessData::Generation(generation),
            tx_hash: raw_tx_hash,
            credential_id,
            credentials,
            default_address,
        },
        runtime_call,
    ))
}

/// Authenticate raw sov-transaction.
pub fn authenticate<
    Accessor: ProvableStateReader<User, Spec = S>,
    S: Spec,
    D: DispatchCall<Spec = S>,
>(
    mut raw_tx: &[u8],
    chain_hash: &[u8; 32],
    state: &mut Accessor,
) -> Result<AuthenticationOutput<S, D::Decodable>, AuthenticationError> {
    let raw_tx_hash = calculate_hash::<Accessor, S>(raw_tx, state)
        .map_err(|e| AuthenticationError::OutOfGas(e.to_string()))?;

    let tx =
        match <Transaction<D, S> as MeteredBorshDeserialize<S>>::deserialize(&mut raw_tx, state) {
            Ok(ok) => ok,

            Err(MeteredBorshDeserializeError::GasError(e)) => {
                return Err(AuthenticationError::OutOfGas(format!(
                    "Transaction deserialization run out of gas {}, tx hash {}",
                    e, raw_tx_hash
                )))
            }
            Err(MeteredBorshDeserializeError::IOError(e)) => {
                return Err(AuthenticationError::FatalError(
                    FatalError::DeserializationFailed(e.to_string()),
                    raw_tx_hash,
                ));
            }
        };

    verify_and_decode_tx::<S, D>(raw_tx_hash, tx, chain_hash, state)
}

#[cfg(feature = "native")]
/// Decode bytes as a Sovereign SDK transaction, returning the message and tx info.
pub fn decode_sov_tx<S: Spec, D: DispatchCall<Spec = S>>(
    mut raw_tx: &[u8],
) -> Result<(D::Decodable, crate::transaction::TransactionWithoutCall<S>), FatalError> {
    let tx = <Transaction<D, S> as MeteredBorshDeserialize<S>>::unmetered_deserialize(&mut raw_tx)
        .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

    let (tx, call) = tx.split();
    Ok((call, tx))
}

/// Calculates the hash of `data` and charges gas.
pub fn calculate_hash<Accessor: ProvableStateReader<User, Spec = S>, S: Spec>(
    data: &[u8],
    accessor: &mut Accessor,
) -> Result<TxHash, GasMeteringError<S::Gas>> {
    let hash =
        MeteredHasher::<Accessor, <S::CryptoSpec as CryptoSpec>::Hasher>::digest(data, accessor)
            .map(TxHash::new)?;

    Ok(hash)
}

/// Helper function to create a `FatalError::DeserializationFailed` authentication error.
pub fn fatal_deserialization_error<
    Accessor: ProvableStateReader<User, Spec = S>,
    S: Spec,
    E: ToString,
>(
    raw_tx: &[u8],
    err: E,
    pre_exec_working_set: &mut Accessor,
) -> AuthenticationError {
    let hash = match calculate_hash::<Accessor, S>(raw_tx, pre_exec_working_set) {
        Ok(hash) => hash,
        Err(err) => return AuthenticationError::OutOfGas(err.to_string()),
    };

    AuthenticationError::FatalError(FatalError::DeserializationFailed(err.to_string()), hash)
}
