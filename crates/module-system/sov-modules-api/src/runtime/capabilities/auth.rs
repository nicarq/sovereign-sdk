//! This module defines abstractions related to transaction authentication and authorization.
//!
//! 1. The [`RuntimeAuthenticator::authenticate`] method accepts bytes and parses them into a structure relevant to a particular authenticator.
//! For example, if the raw bytes form an EVM transaction, the data will be parsed into RLP encoded format followed by an `ECDSA` check.
//! This method returns the following tuple:
//!    - `AuthenticatedTransactionData`: Metadata about the original transaction, such as `chain_id`, `gas_limit`, etc.
//!    - [`RuntimeAuthenticator::Decodable`]: The call message that will be forwarded to the relevant module for execution.
//!    - [`RuntimeAuthenticator::AuthorizationData`]: An associated type used later to authorize the transaction.
//!
//!     The important part is that while the `AuthenticatedTransactionData` and [`RuntimeAuthenticator::Decodable`] are external types that are part of the rollup specification,
//! the [`RuntimeAuthenticator::AuthorizationData`] is created by the [`RuntimeAuthenticator`] implementation, and the stf-blueprint logic is oblivious to it.
//!
//! 2. The [`RuntimeAuthenticator`] contains methods to authorize a transaction.
//! Example:
//! Let's say we have a rollup that supports EVM transactions. At a high level, these are the relevant parts of the workflow:
//!    1. [`RuntimeAuthenticator::authenticate`] authenticates the transaction by checking the ECDSA signature and produces [`RuntimeAuthenticator::AuthorizationData`] that, among other data, contains the transaction nonce.
//!    2. [`RuntimeAuthorization::check_uniqueness`] checks that the nonce is unique.
//!    3. [`RuntimeAuthorization::mark_tx_attempted`] updates the nonce.
//!
//! Notice that in the above example, the concept of the nonce is entirely internal to the implementation of the two traits. We can have other
//! authentication/authorization mechanisms where authentication means something other than a signature check, and the nonce is not used.
//!
//! 3. The [`RuntimeAuthenticator::authenticate_unregistered`] method accepts bytes and parses them
//!    into a structure relevant for registering unregistered sequencers without going through a
//!    registered sequencer. In the normal case the raw bytes will be a Sovereign Rollup
//!    transaction containing a `Register` call message. This method will also accept an unmetered
//!    pre-execution working set that will accumulate costs to charge the sender if execution
//!    succeeds. The implication of this is that misbehaving transaction submissions can't be penalized, thus
//!    there is a need to limit the amount of unregistered transactions we process.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_modules_macros::config_value;
use sov_rollup_interface::common::HexHash;
use sov_rollup_interface::crypto::{CredentialId, PublicKey};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::TxHash;
use thiserror::Error;

use crate::transaction::{
    AuthenticatedTransactionAndRawHash, Credentials, Transaction, TransactionVerificationError,
};
use crate::{
    Context, CryptoSpec, DispatchCall, ExecutionContext, FullyBakedTx, GasMeter,
    MeteredBorshDeserialize, MeteredHasher, PreExecWorkingSet, RawTx, Spec, TxScratchpad,
    UnlimitedGasMeter,
};

/// The chain id of the rollup.
pub const CHAIN_ID: u64 = config_value!("CHAIN_ID");

/// Authenticates raw transactions. Implementations of this trait should provide a way to interpret the raw bytes of the transaction and authenticate it.
/// Typically, the authentication will require checking the signature of the transaction.
pub trait RuntimeAuthenticator<S: Spec> {
    /// Decoded message.
    type Decodable;
    /// A struct that tracks the staked amount of the sequencer and the eventual execution penalities.
    type SequencerStakeMeter: GasMeter<S::Gas>;
    /// The type that is passed to the authorizer.
    type AuthorizationData;

    /// The input to the authenticator
    type Input: BorshDeserialize + BorshSerialize + std::fmt::Debug;

    /// Authenticates raw transaction.
    fn authenticate(
        &self,
        tx: &Self::Input,
        pre_exec_ws: &mut PreExecWorkingSet<S, Self::SequencerStakeMeter>,
    ) -> AuthenticationResult<S, Self::Decodable, Self::AuthorizationData>;
    /// Authenticates raw transactions that are submitted from unregistered sequencers for the
    /// purpose of forced registration (circumventing censorship by currently registered sequencers).
    ///
    /// This function differs to it's registered counterpart in that it typically accepts an
    /// unlimited gas meter to account for the fact there isn't a staked sequencer. It also differs in accepting
    /// a "raw" transaction which hasn't been wrapped in the [`Self::Input`] type. This is because the
    /// only "standard" transactions are allowed to be submitted by unregistered sequencers, so no
    /// additional authentication information is required.
    fn authenticate_unregistered(
        &self,
        tx: &Self::Input,
        state: &mut PreExecWorkingSet<S, UnlimitedGasMeter<S::Gas>>,
    ) -> AuthenticationResult<
        S,
        Self::Decodable,
        Self::AuthorizationData,
        UnregisteredAuthenticationError,
    >;

    /// Encode a standard transaction for the rollup with information describing how to authenticate it.
    fn add_standard_auth(tx: RawTx) -> Self::Input;

    /// Encode the input for the authenticator into a byte array.
    fn encode_athenticator_input(input: &Self::Input) -> FullyBakedTx {
        FullyBakedTx::new(borsh::to_vec(&input).unwrap())
    }

    /// Encode a standard transaction for the rollup with information describing how to authenticate it.
    fn encode_with_standard_auth(tx: RawTx) -> FullyBakedTx {
        Self::encode_athenticator_input(&Self::add_standard_auth(tx))
    }
}

/// Authorizes transactions to be executed.
pub trait RuntimeAuthorization<S: Spec, Da: DaSpec> {
    /// A type-safe struct that should be used to track the staked amount of the sequencer and the eventual execution penalities.
    type SequencerStakeMeter: GasMeter<S::Gas>;

    /// The type used for authorization.
    type AuthorizationData;

    /// Resolves the context for a transaction.
    /// TODO(@preston-evans98): This should be a read-only method `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/384>`
    fn resolve_context(
        &self,
        auth_data: &Self::AuthorizationData,
        sequencer: &Da::Address,
        height: u64,
        pre_exec_ws: &mut PreExecWorkingSet<S, Self::SequencerStakeMeter>,
        context: ExecutionContext,
    ) -> anyhow::Result<Context<S>>;

    /// Resolves the context for an unregistered transaction.
    fn resolve_unregistered_context(
        &self,
        auth_data: &Self::AuthorizationData,
        height: u64,
        state: &mut PreExecWorkingSet<S, UnlimitedGasMeter<S::Gas>>,
        execution_context: ExecutionContext,
    ) -> anyhow::Result<Context<S>>;

    /// Prevents duplicate transactions from running.
    fn check_uniqueness<Meter: GasMeter<S::Gas>>(
        &self,
        auth_data: &Self::AuthorizationData,
        context: &Context<S>,
        pre_exec_ws: &mut PreExecWorkingSet<S, Meter>,
    ) -> anyhow::Result<()>;

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        auth_data: &Self::AuthorizationData,
        sequencer: &Da::Address,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    );
}

/// Result of the authentication.
pub type AuthenticationResult<S, Decodable, Auth, Err = AuthenticationError> =
    Result<(AuthenticatedTransactionAndRawHash<S>, Auth, Decodable), Err>;

/// Error variants that can be raised as a [`AuthenticationError::FatalError`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum FatalError {
    /// Transaction deserialization failed.
    #[error("Transaction deserialization error: {0}")]
    DeserializationFailed(String),
    /// Signature verification failed.
    #[error("Signature verification error: {0}")]
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
    #[error("Transaction decoding error: {0}, tx hash: {1}")]
    MessageDecodingFailed(String, HexHash),
    /// A variant to capture any other fatal error.
    #[error("Other fatal error: {0}")]
    Other(String),
}

/// Authentication error type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum AuthenticationError {
    /// The transaction authentication failed in a way that should have been detected by the sequencer before they accepted the transaction. The sequencer is slashed.
    #[error("Transaction authentication raised a fatal error, error: {0}")]
    FatalError(FatalError),
    /// The transaction authentication returned an error, but including it could have been an honest mistake. The sequencer should be charged enough to cover the cost of checking the transaction but not slashed.
    #[error("Transaction authentication was invalid. error: {0}.")]
    Invalid(
        /// The reason for the penalization.       
        String,
    ),
}

/// Authentication error relating to transactions submitted by an unregistered sequencer for the
/// purpose of direct sequencer registration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum UnregisteredAuthenticationError {
    /// The transaction authentication failed in a way that is unrecoverable.
    #[error("Transaction authentication raised a fatal error, error: {0}")]
    FatalError(FatalError),
    /// The runtime call included in the transaction wasn't a sequencer registry "Register"
    /// message.
    #[error("The runtime call included in the transaction was invalid.")]
    RuntimeCall,
    #[error(
        "The emergency registration transaction did not use the standard authentication mechanism"
    )]
    /// The transaction didn't use the standard authenticator.
    InvalidAuthenticator,
}

impl From<AuthenticationError> for UnregisteredAuthenticationError {
    fn from(value: AuthenticationError) -> Self {
        match value {
            AuthenticationError::FatalError(e) => Self::FatalError(e),
            AuthenticationError::Invalid(e) => Self::FatalError(FatalError::Other(e)),
        }
    }
}
/// Data required to authorize a sov-transaction.
pub struct AuthorizationData<S: Spec> {
    /// The nonce of the transaction.
    pub nonce: u64,

    /// Credential identifier used to retrieve relevant rollup address.
    pub credential_id: CredentialId,

    /// Holds the original credentials to authenticate the transaction and
    /// provides information about which `Authenticator` was used to authenticate the transaction.
    pub credentials: Credentials,

    /// The default address exists only if the original transaction was signed with the default signature schema.
    pub default_address: Option<S::Address>,
}

fn verify_and_decode_tx<S: Spec, D: DispatchCall<Spec = S>>(
    raw_tx_hash: TxHash,
    tx: Transaction<S>,
    meter: &mut impl GasMeter<S::Gas>,
) -> AuthenticationResult<S, D::Decodable, AuthorizationData<S>, AuthenticationError> {
    if tx.details.chain_id != CHAIN_ID {
        return Err(AuthenticationError::FatalError(
            FatalError::InvalidChainId {
                expected: CHAIN_ID,
                got: tx.details.chain_id,
            },
        ));
    }

    tx.verify(meter).map_err(|e| match e {
        TransactionVerificationError::BadSignature(_)
        | TransactionVerificationError::TransactionDeserializationError(_) => {
            AuthenticationError::FatalError(FatalError::SigVerificationFailed(e.to_string()))
        }
        TransactionVerificationError::GasError(_) => AuthenticationError::Invalid(e.to_string()),
    })?;

    let runtime_call = D::decode_call(tx.runtime_msg(), meter).map_err(|e| {
        AuthenticationError::FatalError(FatalError::MessageDecodingFailed(
            e.to_string(),
            raw_tx_hash,
        ))
    })?;

    let pub_key = tx.pub_key().clone();
    let default_address = (&pub_key).into();
    let credential_id = pub_key.credential_id::<<S::CryptoSpec as CryptoSpec>::Hasher>();
    let credentials = Credentials::new(pub_key);
    let nonce = tx.nonce;

    let tx_and_raw_hash = AuthenticatedTransactionAndRawHash {
        raw_tx_hash,
        authenticated_tx: tx.into(),
    };

    Ok((
        tx_and_raw_hash,
        AuthorizationData {
            nonce,
            credential_id,
            credentials,
            default_address: Some(default_address),
        },
        runtime_call,
    ))
}

/// Authenticate raw sov-transaction.
pub fn authenticate<S: Spec, D: DispatchCall<Spec = S>, Meter: GasMeter<S::Gas>>(
    mut raw_tx: &[u8],
    state: &mut PreExecWorkingSet<S, Meter>,
) -> AuthenticationResult<S, D::Decodable, AuthorizationData<S>> {
    let raw_tx_hash = MeteredHasher::<
        S::Gas,
        PreExecWorkingSet<S, Meter>,
        <S::CryptoSpec as CryptoSpec>::Hasher,
    >::digest(raw_tx, state)
    .map(TxHash::new)
    .map_err(|e| AuthenticationError::Invalid(e.to_string()))?;

    let tx = <Transaction<S> as MeteredBorshDeserialize<S::Gas>>::deserialize(&mut raw_tx, state)
        .map_err(|e| {
        AuthenticationError::FatalError(FatalError::DeserializationFailed(e.to_string()))
    })?;

    verify_and_decode_tx::<S, D>(raw_tx_hash, tx, state)
}
