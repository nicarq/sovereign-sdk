//! Methods used to process attestations and challenges.
use core::result::Result::Ok;
use std::fmt::Display;

use sov_modules_api::hooks::TransitionHeight;
use sov_modules_api::optimistic::Attestation;
use sov_modules_api::{
    Gas, InvalidProofError, SerializedAttestation, SerializedChallenge, StateAccessorError,
    StateTransitionPublicData, TxState, Zkvm,
};
use sov_state::storage::{Storage, StorageProof};
use thiserror::Error;
use tracing::error;

use super::call::SlashingReason;
use crate::AttesterIncentives;

/// Error raised while processing the attester incentives.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProcessAttestationErrors<AccessorError> {
    #[error("Unable to deserialize the attestation.")]
    /// Unable to deserialize the attestation.
    InvalidAttestationFormat,

    #[error("Attester slashed: {0}")]
    /// Attester slashed
    AttesterSlashed(SlashingReason),

    #[error("Attester is not bonded at the time of the transaction")]
    /// Attester is not bonded at the time of the transaction
    AttesterNotBonded,

    #[error("Invalid bonding proof")]
    /// The bonding proof was invalid
    InvalidBondingProof,

    #[error("The bond is not a 64-bit number")]
    /// The bond is not a 64-bit number
    InvalidBondFormat,

    #[error("Transition invariant isn't respected")]
    /// Transition invariant isn't respected
    InvalidTransitionInvariant,

    #[error("Error occurred when trying to reward a user. {0}. This is a bug.")]
    /// An error occurred when transferred funds
    RewardTransferFailure(String),

    #[error("Error occurred when accessing the state, error: {0}")]
    /// An error occurred when accessing the state
    StateAccessError(#[from] AccessorError),
}

impl<AccessorError: Display> From<ProcessAttestationErrors<AccessorError>> for InvalidProofError {
    fn from(error: ProcessAttestationErrors<AccessorError>) -> Self {
        match error {
            ProcessAttestationErrors::AttesterSlashed(reason) => {
                InvalidProofError::ProverSlashed(format!("{}", reason))
            }
            ProcessAttestationErrors::InvalidAttestationFormat
            | ProcessAttestationErrors::AttesterNotBonded
            | ProcessAttestationErrors::InvalidBondingProof
            | ProcessAttestationErrors::InvalidTransitionInvariant
            | ProcessAttestationErrors::InvalidBondFormat => {
                InvalidProofError::PreconditionNotMet(format!("{}", error))
            }
            ProcessAttestationErrors::RewardTransferFailure(e) => {
                InvalidProofError::RewardFailure(e)
            }
            ProcessAttestationErrors::StateAccessError(e) => {
                InvalidProofError::StateAccess(e.to_string())
            }
        }
    }
}

/// Error raised while processing the attester incentives.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProcessChallengeErrors<AccessorError> {
    #[error("Challenger slashed")]
    /// The user was slashed. Reason specified by [`SlashingReason`]
    ChallengerSlashed(#[source] SlashingReason),

    #[error("Challenger is not bonded at the time of the transaction")]
    /// User is not bonded at the time of the transaction
    ChallengerNotBonded,

    #[error("Error occurred when trying to reward a user: {0}. This is a bug.")]
    /// An error occurred when transferred funds
    RewardTransferFailure(String),

    #[error("Error occurred when accessing the state, error: {0}")]
    /// An error occurred when accessing the state
    StateAccessError(#[from] AccessorError),
}

impl<AccessorError: Display> From<ProcessChallengeErrors<AccessorError>> for InvalidProofError {
    fn from(error: ProcessChallengeErrors<AccessorError>) -> Self {
        match error {
            ProcessChallengeErrors::ChallengerSlashed(reason) => {
                InvalidProofError::ProverSlashed(reason.to_string())
            }
            ProcessChallengeErrors::ChallengerNotBonded => {
                InvalidProofError::PreconditionNotMet(format!("{}", error))
            }
            ProcessChallengeErrors::RewardTransferFailure(e) => InvalidProofError::RewardFailure(e),
            ProcessChallengeErrors::StateAccessError(e) => {
                InvalidProofError::StateAccess(e.to_string())
            }
        }
    }
}

impl<AccessorError> ProcessChallengeErrors<AccessorError> {
    pub(crate) fn slashed(value: SlashingReason) -> Self {
        Self::ChallengerSlashed(value)
    }
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
    pub fn process_attestation(
        &self,
        sender: &S::Address,
        serialized_attestation: SerializedAttestation,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<
        Attestation<
            Da::SlotHash,
            <S::Storage as Storage>::Root,
            StorageProof<<S::Storage as Storage>::Proof>,
        >,
        ProcessAttestationErrors<StateAccessorError<S::Gas>>,
    > {
        let attestation = serialized_attestation.to_attestation().map_err(|e| {
            error!(error = ?e, "Unable to deserialize the attestation.");
            ProcessAttestationErrors::InvalidAttestationFormat
        })?;
        self.process_attestation_helper(sender, &attestation, state)?;
        Ok(attestation)
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn process_attestation_helper(
        &self,
        sender: &S::Address,
        attestation: &Attestation<
            Da::SlotHash,
            <S::Storage as Storage>::Root,
            StorageProof<<S::Storage as Storage>::Proof>,
        >,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<(), ProcessAttestationErrors<StateAccessorError<S::Gas>>> {
        // We first need to check that the attester is still in the bonding set
        if self.bonded_attesters.get(sender, state)?.is_none() {
            return Err(ProcessAttestationErrors::AttesterNotBonded);
        }

        // If the bonding proof in the attestation is invalid, light clients will ignore the attestation. In that case, we should too.
        self.check_bonding_proof(sender, attestation, state)?;

        // We suppose that these values are always defined, otherwise we panic
        let last_attested_height = self
            .maximum_attested_height
            .get(state)?
            .expect("The maximum attested height should be set at genesis");
        let current_finalized_height = self
            .light_client_finalized_height
            .get(state)?
            .expect("The light client finalized height should be set at genesis");
        let finality = self
            .rollup_finality_period
            .get(state)?
            .expect("The rollup finality period should be set at genesis");

        assert!(
            current_finalized_height <= last_attested_height,
            "The last attested height should always be below the current finalized height."
        );

        // Update the max_attested_height in case the blocks have already been finalized
        let new_height_to_attest = last_attested_height
            .checked_add(1)
            .expect("reached end of the chain");

        // Minimum height at which the proof of bond can be valid
        let min_height = new_height_to_attest.saturating_sub(finality);

        // We have to check the following order invariant is respected:
        // (height to attest - finality) <= bonding_proof.transition_num <= height to attest
        //
        // Which with our variable gives:
        // min_height <= bonding_proof.transition_num <= new_height_to_attest
        // If this invariant is respected, we can be sure that the attester was bonded at new_height_to_attest.
        if !(min_height <= attestation.proof_of_bond.claimed_transition_num
            && attestation.proof_of_bond.claimed_transition_num <= new_height_to_attest)
        {
            return Err(ProcessAttestationErrors::InvalidTransitionInvariant);
        }

        // From this point below, the attester has been correctly authenticated -
        // any error constitutes a slashable offense which *needs to be reflected in the state*.
        // Hence we don't want to return an error after this point, but rather slash the attester and exit gracefully.

        // First compare the initial hashes
        if let Err(err) = self.check_initial_hash(
            attestation.proof_of_bond.claimed_transition_num,
            sender,
            attestation,
            state,
        ) {
            error!(
                error = ?err,
                ?attestation,
                "Error raised when checking initial hashes for attestation");
            return Ok(());
        }

        // Then compare the transition
        if let Err(err) = self.check_transition(
            attestation.proof_of_bond.claimed_transition_num,
            sender,
            attestation,
            state,
        ) {
            error!(
                error = ?err,
                ?attestation,
                "Error raised when checking the transition for attestation");
            return Ok(());
        }

        // Now we have to check whether the claimed_transition_num is the max_attested_height.
        // If so, update the maximum attested height and reward the sender
        if attestation.proof_of_bond.claimed_transition_num == new_height_to_attest {
            // We reward the attester with the amount of gas used for the transition.
            let transition = self
                .chain_state
                .get_historical_transitions(new_height_to_attest, state)?
                .expect("The transition should exist. The check has been done above");

            let reward = transition.gas_used().value(transition.gas_price());

            // Update the maximum attested height
            self.maximum_attested_height
                .set(&(new_height_to_attest), state)?;

            self.transfer_tokens_to_sender(sender, self.burn_rate().apply(reward), state)
                .map_err(|err| {
                    error!(
                        error = ?err,
                        "Error raised transferring reward to the attester");
                    ProcessAttestationErrors::RewardTransferFailure(err.to_string())
                })?;
        }

        // Then we can optimistically process the transaction
        Ok(())
    }

    /// Try to process a zk proof if the challenger is bonded.
    /// Same comment as above for the [`AttesterIncentives::process_attestation`] method: if we have a slashable
    /// offense, we want to be able to exit gracefully.

    #[allow(clippy::type_complexity)]
    pub fn process_challenge(
        &self,
        sender: &S::Address,
        serialized_challenge: &SerializedChallenge,
        transition_num: TransitionHeight,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<
        Option<StateTransitionPublicData<S::Address, Da, <S::Storage as Storage>::Root>>,
        ProcessChallengeErrors<StateAccessorError<S::Gas>>,
    > {
        self.process_challenge_helper(
            sender,
            &serialized_challenge.raw_challenge,
            transition_num,
            state,
        )
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn process_challenge_helper(
        &self,
        sender: &S::Address,
        proof: &[u8],
        transition_num: TransitionHeight,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<
        Option<StateTransitionPublicData<S::Address, Da, <S::Storage as Storage>::Root>>,
        ProcessChallengeErrors<StateAccessorError<S::Gas>>,
    > {
        // Get the challenger's old balance.
        // Revert if they aren't bonded
        let old_balance = self
            .bonded_challengers
            .get_or_err(sender, state)?
            .map_err(|_| ProcessChallengeErrors::ChallengerNotBonded)?;

        // Check that the challenger has enough balance to process the proof.
        let minimum_bond = self
            .minimum_challenger_bond
            .get(state)?
            .expect("Should be set at genesis");

        if old_balance < minimum_bond {
            return Err(ProcessChallengeErrors::ChallengerNotBonded);
        }

        let code_commitment = self
            .chain_state
            .inner_code_commitment(state)?
            .expect("Should be set at genesis");

        // Find the faulty attestation pool and get the associated reward
        let attestation_reward: u64 = match self
            .bad_transition_pool
            .get_or_err(&transition_num, state)?
        {
            Ok(reward) => reward,
            Err(err) => {
                error!(error = ?err, "Challenger slashed");
                self.slash_challenger_burn_reward(
                    sender,
                    SlashingReason::NoInvalidTransition,
                    state,
                )?;

                return Ok(None);
            }
        };

        let public_outputs_opt = <S::InnerZkvm as Zkvm>::verify::<
            StateTransitionPublicData<S::Address, Da, <S::Storage as Storage>::Root>,
        >(proof, &code_commitment)
        .map_err(|e| anyhow::format_err!("{:?}", e));

        // Don't return an error for invalid proofs - those are expected and shouldn't cause reverts.
        match public_outputs_opt {
            Ok(public_output) => {
                // We have to perform the checks to ensure that the challenge is valid while the attestation isn't.
                if let Err(err) = self.check_challenge_outputs_against_transition(
                    &public_output,
                    transition_num,
                    state,
                ) {
                    if let ProcessChallengeErrors::ChallengerSlashed(err) = err {
                        self.slash_challenger_burn_reward(sender, err, state)?;
                        return Ok(None);
                    }

                    return Err(err);
                };

                // Reward the sender
                self.transfer_tokens_to_sender(
                    sender,
                    self.burn_rate().apply(attestation_reward),
                    state,
                )
                .map_err(|err| {
                    error!(
                            error = ?err,
                            "Error raised transferring reward to the challenger");
                    ProcessChallengeErrors::RewardTransferFailure(err.to_string())
                })?;

                // Now remove the bad transition from the pool
                self.bad_transition_pool.remove(&transition_num, state)?;

                Ok(Some(public_output))
            }
            Err(_err) => {
                // Slash the challenger
                self.slash_challenger_burn_reward(
                    sender,
                    SlashingReason::InvalidProofOutputs,
                    state,
                )?;
                Ok(None)
            }
        }
    }
}
