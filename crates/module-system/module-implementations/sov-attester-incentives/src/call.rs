use core::result::Result::Ok;
use std::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};
use derivative::Derivative;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sov_modules_api::hooks::TransitionHeight;
use sov_modules_api::optimistic::Attestation;
use sov_modules_api::DaSpec;
use sov_state::storage::{Storage, StorageProof};
use thiserror::Error;
use tracing::error;

use crate::Amount;

/// A wrapper for attestations which implements `borsh` serialization. This is necessary since
/// Attestations are treated as `CallMessage`s, and we only support borsh encoding for transactions.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrappedAttestation<Da: DaSpec, StorageProof, Root> {
    #[serde(
        bound = "Da::SlotHash: Serialize + DeserializeOwned, StorageProof: Serialize + DeserializeOwned, Root: Serialize + DeserializeOwned"
    )]
    /// The inner attestation
    pub inner: Attestation<Da, StorageProof, Root>,
}

impl<Da: DaSpec, StorageProof: Debug, Root: Debug> Debug
    for WrappedAttestation<Da, StorageProof, Root>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WrappedAttestation")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<Da: DaSpec, StorageProof, Root> From<Attestation<Da, StorageProof, Root>>
    for WrappedAttestation<Da, StorageProof, Root>
{
    fn from(value: Attestation<Da, StorageProof, Root>) -> Self {
        Self { inner: value }
    }
}

impl<Da: DaSpec, StorageProof: Serialize, Root: Serialize> BorshSerialize
    for WrappedAttestation<Da, StorageProof, Root>
{
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        // TODO: Implement bcs `to_writer`
        let value = bcs::to_bytes(&self.inner).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, "Failed to serialize attestation")
        })?;
        writer.write_all(&value)?;
        Ok(())
    }
}

impl<
        Da: DaSpec,
        StorageProof: Serialize + DeserializeOwned,
        Root: Serialize + DeserializeOwned,
    > BorshDeserialize for WrappedAttestation<Da, StorageProof, Root>
{
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        bcs::from_reader(reader)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    fn deserialize(buf: &mut &[u8]) -> std::io::Result<Self> {
        bcs::from_bytes(buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }
}

/// This enumeration represents the available call messages for interacting with the `AttesterIncentives` module.
#[derive(Derivative, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[derivative(
    PartialEq(bound = "<S::Storage as Storage>::Proof: PartialEq + Eq"),
    Eq(bound = "<S::Storage as Storage>::Proof: PartialEq + Eq")
)]
pub enum CallMessage<S: sov_modules_api::Spec, Da: DaSpec> {
    /// Bonds an attester, the parameter is the bond amount
    BondAttester(Amount),
    /// Start the first phase of the two-phase unbonding process
    BeginUnbondingAttester,
    /// Finish the two phase unbonding
    EndUnbondingAttester,
    /// Bonds a challenger, the parameter is the bond amount
    BondChallenger(Amount),
    /// Unbonds a challenger
    UnbondChallenger,
    /// Processes an attestation.
    ProcessAttestation(
        #[allow(clippy::type_complexity)]
        WrappedAttestation<
            Da,
            StorageProof<<S::Storage as Storage>::Proof>,
            <S::Storage as Storage>::Root,
        >,
    ),
    /// Processes a challenge. The challenge is encoded as a [`Vec<u8>`]. The second parameter is the transition number
    ProcessChallenge(Vec<u8>, TransitionHeight),
}

// Manually implement Debug to remove spurious Debug bound on S::Storage
impl<S: sov_modules_api::Spec, Da: DaSpec> Debug for CallMessage<S, Da> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BondAttester(arg0) => f.debug_tuple("BondAttester").field(arg0).finish(),
            Self::BeginUnbondingAttester => write!(f, "BeginUnbondingAttester"),
            Self::EndUnbondingAttester => write!(f, "EndUnbondingAttester"),
            Self::BondChallenger(arg0) => f.debug_tuple("BondChallenger").field(arg0).finish(),
            Self::UnbondChallenger => write!(f, "UnbondChallenger"),
            Self::ProcessAttestation(arg0) => {
                f.debug_tuple("ProcessAttestation").field(arg0).finish()
            }
            Self::ProcessChallenge(arg0, arg1) => f
                .debug_tuple("ProcessChallenge")
                .field(arg0)
                .field(arg1)
                .finish(),
        }
    }
}

#[derive(
    Debug,
    Error,
    PartialEq,
    Eq,
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Copy,
    Serialize,
    Deserialize,
)]
/// Error type that explains why a user is slashed
pub enum SlashingReason {
    #[error("Transition isn't found")]
    /// The specified transition does not exist
    TransitionNotFound,

    #[error("The attestation does not contain the right block hash and post-state transition")]
    /// The specified transition is invalid (block hash, post-root hash or validity condition)
    TransitionInvalid,

    #[error("The initial hash of the transition is invalid")]
    /// The initial hash of the transition is invalid.
    InvalidInitialHash,

    #[error("The proof opening raised an error")]
    /// The proof verification raised an error
    InvalidProofOutputs,

    #[error("No invalid transition to challenge")]
    /// No invalid transition to challenge.
    NoInvalidTransition,
}

/// Error raised while processing the attester incentives
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AttesterIncentiveErrors<AccessorError> {
    #[error("The sender key doesn't match the attester key provided in the proof")]
    /// The sender key doesn't match the attester key provided in the proof
    InvalidSender,

    #[error("Attester is unbonding")]
    /// The attester is in the first unbonding phase
    AttesterIsUnbonding,

    #[error("User is not trying to unbond at the time of the transaction")]
    /// User is not trying to unbond at the time of the transaction
    AttesterIsNotUnbonding,

    #[error("The first phase of unbonding has not been finalized")]
    /// The attester is trying to finish the two-phase unbonding too soon
    UnbondingNotFinalized,

    #[error("Error occurred when transferred bonding funds. The user's account may not have enough funds")]
    /// An error occurred when transferred funds
    BondTransferFailure,

    #[error(
        "Error occurred when trying to reward a user. The `AttesterIncentives` module may not have enough funds. This is a bug."
    )]
    /// An error occurred when transferred funds
    RewardTransferFailure,

    /// An error occurred when accessing the state
    #[error("Error occurred when accessing the state, error: {0}")]
    StateAccessError(#[from] AccessorError),
}
