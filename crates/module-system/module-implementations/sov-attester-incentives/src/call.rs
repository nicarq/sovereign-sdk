use core::result::Result::Ok;
use std::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};
use derivative::Derivative;
use serde::{Deserialize, Serialize};
use sov_modules_api::hooks::TransitionHeight;
use sov_modules_api::optimistic::Attestation;
use sov_modules_api::{CallResponse, Context, DaSpec, StateAccessorError, TxState};
use sov_state::storage::{Storage, StorageProof};
use thiserror::Error;
use tracing::error;

use crate::{Amount, AttesterIncentives, ProcessAttestationErrors, ProcessChallengeErrors};

/// This enumeration represents the available call messages for interacting with the `AttesterIncentives` module.
#[derive(
    Derivative,
    BorshDeserialize,
    BorshSerialize,
    Serialize,
    Deserialize,
    Clone,
    sov_modules_api::macros::UniversalWallet,
)]
#[derivative(
    PartialEq(bound = "<S::Storage as Storage>::Proof: PartialEq + Eq"),
    Eq(bound = "<S::Storage as Storage>::Proof: PartialEq + Eq")
)]
pub enum CallMessage<S: sov_modules_api::Spec, Da: DaSpec> {
    /// Register an attester, the parameter is the bond amount
    RegisterAttester(Amount),
    /// Start the first phase of the two-phase exit process
    BeginExitAttester,
    /// Finish the two phase exit
    ExitAttester,
    /// Register a challenger, the parameter is the bond amount
    RegisterChallenger(Amount),
    /// Exit a challenger
    ExitChallenger,
    /// Processes an attestation.
    ProcessAttestation(
        #[allow(clippy::type_complexity)]
        Attestation<
            Da,
            StorageProof<<S::Storage as Storage>::Proof>,
            <S::Storage as Storage>::Root,
        >,
    ),
    /// Processes a challenge. The challenge is encoded as a [`Vec<u8>`]. The second parameter is the transition number
    ProcessChallenge(Vec<u8>, TransitionHeight),
    /// Increases the balance of the attester.    
    DepositAttester(Amount),
}

// Manually implement Debug to remove spurious Debug bound on S::Storage
impl<S: sov_modules_api::Spec, Da: DaSpec> Debug for CallMessage<S, Da> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RegisterAttester(arg0) => f.debug_tuple("RegisterAttester").field(arg0).finish(),
            Self::DepositAttester(arg0) => f.debug_tuple("DepositAttester").field(arg0).finish(),
            Self::BeginExitAttester => write!(f, "BeginExitAttester"),
            Self::ExitAttester => write!(f, "ExitAttester"),
            Self::RegisterChallenger(arg0) => {
                f.debug_tuple("RegisterChallenger").field(arg0).finish()
            }
            Self::ExitChallenger => write!(f, "ExitChallenger"),
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

impl<S, Da> AttesterIncentives<S, Da>
where
    S: sov_modules_api::Spec,
    Da: sov_modules_api::DaSpec,
{
    /// Try to process an attestation if the attester is bonded.
    /// This function returns an error (hence ignores the transaction) when the attester is not bonded
    /// or when the module is unable to verify the bonding proof.
    #[allow(clippy::type_complexity)]
    pub(crate) fn process_attestation_call(
        &self,
        context: &Context<S>,
        attestation: Attestation<
            Da,
            StorageProof<<S::Storage as Storage>::Proof>,
            <S::Storage as Storage>::Root,
        >,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<CallResponse, ProcessAttestationErrors<StateAccessorError<S::Gas>>> {
        self.process_attestation(context, attestation, state)?;
        Ok(sov_modules_api::CallResponse::default())
    }

    pub(crate) fn process_challenge_call(
        &self,
        context: &Context<S>,
        proof: &[u8],
        transition_num: &TransitionHeight,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<CallResponse, ProcessChallengeErrors<StateAccessorError<S::Gas>>> {
        self.process_challenge(context, proof, transition_num, state)?;
        Ok(sov_modules_api::CallResponse::default())
    }
}
