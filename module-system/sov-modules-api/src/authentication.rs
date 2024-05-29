use borsh::BorshDeserialize;
use sov_rollup_interface::crypto::{CredentialId, PublicKey};
use sov_rollup_interface::zk::CryptoSpec;

use crate::capabilities::{AuthenticationError, FatalError, RawTx};
use crate::digest::Digest;
use crate::transaction::{AuthenticatedTransactionAndRawHash, Credentials, Transaction};
use crate::{DispatchCall, GasMeter, PreExecWorkingSet, Spec};

/// Result of the authentication.
pub type AuthenticationResult<S, Decodable, Auth> =
    Result<(AuthenticatedTransactionAndRawHash<S>, Auth, Decodable), AuthenticationError>;

/// A single rollup can support several authentication mechanisms.
/// For example, within the same rollup, some transactions can be signed with the SignatureA scheme and others with the SignatureB scheme.
/// The Authenticator trait makes it possible to abstract away the details of how the transaction should be validated.
pub trait Authenticator: Send + Sync + 'static {
    /// The rollup Spec.
    type Spec: Spec;
    /// CallMessage dispatcher.
    type DispatchCall: DispatchCall;
    /// The type that is passed to the authorizer.
    type AuthorizationData;

    /// Accepts raw tx and interprets it as a transaction, performing validation relevant to a particular authentication scheme.
    /// The `stake_meter` is used to track and accumulate potential penalties for the sequencer.
    fn authenticate<Meter: GasMeter<<Self::Spec as Spec>::Gas>>(
        raw_tx: &[u8],
        stake_meter: &mut PreExecWorkingSet<Self::Spec, Meter>,
    ) -> AuthenticationResult<
        Self::Spec,
        <Self::DispatchCall as DispatchCall>::Decodable,
        Self::AuthorizationData,
    >;

    /// Encodes transaction bytes using a particular authenticator.
    fn encode(tx: Vec<u8>) -> Result<RawTx, anyhow::Error>;
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

// Authenticate raw sov-transaction.
pub fn authenticate<S: Spec, D: DispatchCall, Meter: GasMeter<S::Gas>>(
    mut raw_tx: &[u8],
    stake_meter: &mut PreExecWorkingSet<S, Meter>,
) -> AuthenticationResult<S, D::Decodable, AuthorizationData<S>> {
    let raw_tx_hash = <S::CryptoSpec as CryptoSpec>::Hasher::digest(raw_tx).into();

    // TODO(@theochap): Charge gas for deserialization.

    let tx = Transaction::<S>::deserialize(&mut raw_tx).map_err(|e| {
        AuthenticationError::FatalError(FatalError::DeserializationFailed(e.to_string()))
    })?;

    stake_meter.charge_gas(&tx.gas_fixed_cost()).map_err(|e| {
        AuthenticationError::Invalid(format!(
            "Failed to reserve gas for signature checks from the sequencer's stake: {:?}",
            e
        ))
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
