//! Methods used to process attestations and challenges.
use core::result::Result::Ok;

use anyhow::ensure;
use borsh::BorshDeserialize;
use sov_bank::{BurnRate, Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::hooks::TransitionHeight;
use sov_modules_api::macros::config_value;
use sov_modules_api::optimistic::Attestation;
use sov_modules_api::{
    CallResponse, Context, EventEmitter, Gas, StateAccessor, StateAccessorError,
    StateTransitionPublicData, StateWriter, TxState, Zkvm,
};
use sov_state::storage::{SlotKey, SlotValue, Storage, StorageProof};
use sov_state::User;
use thiserror::Error;
use tracing::{debug, error};

use super::call::{SlashingReason, WrappedAttestation};
use crate::{AttesterIncentives, Event};

/// Error raised while processing the attester incentives.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProcessAttestationErrors<AccessorError> {
    #[error("Attester is not bonded at the time of the transaction")]
    /// Attester is not bonded at the time of the transaction
    AttesterSlashed(SlashingReason),

    #[error("Attester slashed")]
    /// Attester slashed
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

    #[error(
        "Error occurred when trying to reward a user. The `AttesterIncentives` module may not have enough funds. This is a bug."
    )]
    /// An error occurred when transferred funds
    RewardTransferFailure,

    #[error("Error occurred when accessing the state, error: {0}")]
    /// An error occurred when accessing the state
    StateAccessError(#[from] AccessorError),
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

    #[error(
        "Error occurred when trying to reward a user. The `AttesterIncentives` module may not have enough funds. This is a bug."
    )]
    /// An error occurred when transferred funds
    RewardTransferFailure,

    #[error("Error occurred when accessing the state, error: {0}")]
    /// An error occurred when accessing the state
    StateAccessError(#[from] AccessorError),
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
    /// Returns the burn rate for the reward
    pub fn burn_rate(&self) -> BurnRate {
        const PERCENT_BASE_FEE_TO_BURN: u8 = config_value!("PERCENT_BASE_FEE_TO_BURN");

        BurnRate::new_unchecked(PERCENT_BASE_FEE_TO_BURN)
    }

    /// Verifies the provided proof, returning its underlying storage value, if present.
    pub fn verify_proof(
        &self,
        state_root: <S::Storage as Storage>::Root,
        proof: StorageProof<<S::Storage as Storage>::Proof>,
        expected_key: &S::Address,
    ) -> Result<Option<SlotValue>, anyhow::Error> {
        let (storage_key, storage_value) = S::Storage::open_proof(state_root, proof)?;
        let prefix = self.bonded_attesters.prefix();
        let codec = self.bonded_attesters.codec();

        // We have to check that the storage key is the same as the external key
        ensure!(
            storage_key == SlotKey::new(prefix, expected_key, codec),
            "The storage key from the proof doesn't match the expected storage key."
        );

        Ok(storage_value)
    }

    /// A helper function that simply slashes an attester and returns a reward value
    fn slash_attester<TxStateAccessor: TxState<S>>(
        &self,
        user: &S::Address,
        reason: SlashingReason,
        state: &mut TxStateAccessor,
    ) -> Result<u64, <TxStateAccessor as StateWriter<User>>::Error> {
        // We have to remove the attester from the unbonding set
        // to prevent him from skipping the first phase
        // unbonding if he bonds himself again.
        self.unbonding_attesters.remove(user, state)?;
        let bonded_set = &self.bonded_attesters;

        // We have to deplete the attester's bonded account, it amounts to removing the attester from the bonded set
        let reward = bonded_set.get(user, state)?.unwrap_or_default();
        bonded_set.remove(user, state)?;

        // We raise an event
        self.emit_event(
            state,
            Event::<S>::UserSlashed {
                address: user.clone(),
                reason,
            },
        );

        Ok(reward)
    }

    fn slash_attester_burn_reward(
        &self,
        user: &S::Address,
        reason: SlashingReason,
        state: &mut impl TxState<S>,
    ) -> Result<(), ProcessAttestationErrors<StateAccessorError<S::Gas>>> {
        if let Err(e) = self.slash_attester(user, reason, state) {
            error!(
                error = ?e,
                "Error raised when trying to slash the attester. Attester not slashed and transaction reverted"
            );
            return Err(e.into());
        };
        Ok(())
    }

    fn slash_challenger_burn_reward(
        &self,
        user: &S::Address,
        reason: SlashingReason,
        state: &mut impl TxState<S>,
    ) -> Result<(), ProcessChallengeErrors<StateAccessorError<S::Gas>>> {
        self.bonded_challengers.remove(user, state)?;

        self.emit_event(
            state,
            Event::UserSlashed {
                address: user.clone(),
                reason,
            },
        );

        error!(
            error = ?reason,
            "The user was slashed");

        Ok(())
    }

    /// A helper function that is used to slash an attester, and put the associated attestation in the slashed pool
    fn slash_and_invalidate_attestation<TxStateAccessor: TxState<S>>(
        &self,
        attester: &S::Address,
        height: TransitionHeight,
        reason: SlashingReason,
        state: &mut TxStateAccessor,
    ) -> Result<
        ProcessAttestationErrors<StateAccessorError<S::Gas>>,
        <TxStateAccessor as StateWriter<User>>::Error,
    > {
        let reward = self.slash_attester(attester, reason, state)?;

        let curr_reward_value = self
            .bad_transition_pool
            .get(&height, state)?
            .unwrap_or_default();

        let new_value = curr_reward_value.saturating_add(reward);
        self.bad_transition_pool.set(&height, &new_value, state)?;

        Ok(ProcessAttestationErrors::AttesterSlashed(reason))
    }

    pub(crate) fn transfer_tokens_to_sender(
        &self,
        context: &Context<S>,
        amount: u64,
        state: &mut impl StateAccessor,
    ) -> anyhow::Result<()> {
        let coins = Coins {
            token_id: GAS_TOKEN_ID,
            amount,
        };

        // The reward tokens are unlocked from the module's id.
        self.bank
            .transfer_from(self.id.to_payable(), context.sender(), coins, state)?;

        Ok(())
    }

    /// The bonding proof is now a proof that an attester was bonded during the last `finality_period` range.
    /// The proof must refer to a valid state of the rollup. The initial root hash must represent a state between
    /// the bonding proof one and the current state.
    #[allow(clippy::type_complexity)]
    fn check_bonding_proof(
        &self,
        context: &Context<S>,
        attestation: &Attestation<
            Da,
            StorageProof<<S::Storage as Storage>::Proof>,
            <S::Storage as Storage>::Root,
        >,
        state: &mut impl TxState<S>,
    ) -> Result<(), ProcessAttestationErrors<StateAccessorError<S::Gas>>> {
        let bonding_root = {
            // If we cannot get the transition before the current one, it means that we are trying
            // to get the genesis state root
            let transition_height = TransitionHeight::from(
                attestation
                    .proof_of_bond
                    .claimed_transition_num
                    .checked_sub(1)
                    .expect("The transition height should be greater than 1"),
            );

            if let Some(transition) = self
                .chain_state
                .get_historical_transitions(transition_height, state)?
            {
                transition.post_state_root().clone()
            } else {
                self.chain_state
                    .get_genesis_hash(state)?
                    .expect("The genesis hash should be set at genesis")
            }
        };

        // This proof checks that the attester was bonded at the given transition num
        let bond_opt = self
            .verify_proof(
                bonding_root,
                attestation.proof_of_bond.proof.clone(),
                context.sender(),
            )
            .map_err(|_err| ProcessAttestationErrors::InvalidBondingProof)?;

        let bond = bond_opt.ok_or(ProcessAttestationErrors::AttesterNotBonded)?;
        let bond: u64 = BorshDeserialize::deserialize(&mut bond.value())
            .map_err(|_err| ProcessAttestationErrors::InvalidBondFormat)?;

        let minimum_bond = self
            .minimum_attester_bond
            .get_or_err(state)?
            .expect("The minimum bond should be set at genesis");

        // We then have to check that the bond was greater than the minimum bond
        if bond < minimum_bond {
            return Err(ProcessAttestationErrors::AttesterNotBonded);
        }

        Ok(())
    }

    #[allow(clippy::type_complexity)]
    fn check_transition(
        &self,
        claimed_transition_height: TransitionHeight,
        attester: &S::Address,
        attestation: &Attestation<
            Da,
            StorageProof<<S::Storage as Storage>::Proof>,
            <S::Storage as Storage>::Root,
        >,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, ProcessAttestationErrors<StateAccessorError<S::Gas>>> {
        if let Some(curr_tx) = self
            .chain_state
            .get_historical_transitions(claimed_transition_height, state)?
        {
            // We first need to compare the initial block hash to the previous post state root
            if !curr_tx.compare_hashes(&attestation.slot_hash, &attestation.post_state_root) {
                debug!(
                    claimed_transition_height,
                    attestation_slot_hash = ?attestation.slot_hash,
                    attestation_post_state = ?attestation.post_state_root,
                    curr_tx_slot_hash = ?curr_tx.slot_hash(),
                    curr_tx_state_root = ?curr_tx.post_state_root(),
                    "The attestation has an invalid block hash or post state root");
                // Check if the attestation has the same slot_hash and post_state_root as the actual transition
                // that we found in state. If not, slash the attester.
                // If so, the attestation is valid, so return Ok
                match self.slash_and_invalidate_attestation(
                    attester,
                    claimed_transition_height,
                    SlashingReason::TransitionInvalid,
                    state,
                ) {
                    Err(e) => {
                        error!(
                            error = ?e,
                            "An error occurred while slashing the attester. Attester not slashed and transaction reverted");
                        return Err(e.into());
                    }

                    Ok(e) => {
                        self.emit_event(
                            state,
                            Event::UserSlashed {
                                address: attester.clone(),
                                reason: SlashingReason::TransitionInvalid,
                            },
                        );

                        return Err(e);
                    }
                }
            }
            Ok(CallResponse::default())
        } else {
            // Case where we cannot get the transition from the chain state historical transitions.
            self.slash_attester_burn_reward(attester, SlashingReason::TransitionNotFound, state)?;
            Ok(CallResponse::default())
        }
    }

    #[allow(clippy::type_complexity)]
    fn check_initial_hash(
        &self,
        claimed_transition_height: TransitionHeight,
        attester: &S::Address,
        attestation: &Attestation<
            Da,
            StorageProof<<S::Storage as Storage>::Proof>,
            <S::Storage as Storage>::Root,
        >,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<CallResponse, ProcessAttestationErrors<StateAccessorError<S::Gas>>> {
        // Normal state
        if let Some(transition) = self
            .chain_state
            .get_historical_transitions(claimed_transition_height.saturating_sub(1), state)?
        {
            if transition.post_state_root() != &attestation.initial_state_root {
                // The initial root hashes don't match, just slash the attester
                self.slash_attester_burn_reward(
                    attester,
                    SlashingReason::InvalidInitialHash,
                    state,
                )?;
                return Ok(CallResponse::default());
            }
        } else {
            // Genesis state
            // We can assume that the genesis hash is always set, otherwise we need to panic.
            // We don't need to prove that the attester was bonded, simply need to check that the current bond is higher than the
            // minimal bond and that the attester is not unbonding

            // We add a check here that the claimed transition height is the same as the genesis height.
            let genesis_height = 0;
            let previous = claimed_transition_height
                .checked_sub(1)
                .expect("Transition height must be > 0");
            if genesis_height != previous {
                self.slash_attester_burn_reward(
                    attester,
                    SlashingReason::TransitionNotFound,
                    state,
                )?;

                return Ok(CallResponse::default());
            }

            if self
                .chain_state
                .get_genesis_hash(state)?
                .expect("The initial hash should be set")
                != attestation.initial_state_root
            {
                // Slash the attester, and burn the fees
                self.slash_attester_burn_reward(
                    attester,
                    SlashingReason::InvalidInitialHash,
                    state,
                )?;

                return Ok(CallResponse::default());
            }
            // Normal state
        }

        Ok(CallResponse::default())
    }

    /// Try to process an attestation if the attester is bonded.
    /// This function returns an error (hence ignores the transaction) when the attester is not bonded
    /// or when the module is unable to verify the bonding proof.
    #[allow(clippy::type_complexity)]
    pub(crate) fn process_attestation(
        &self,
        context: &Context<S>,
        attestation: WrappedAttestation<
            Da,
            StorageProof<<S::Storage as Storage>::Proof>,
            <S::Storage as Storage>::Root,
        >,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<CallResponse, ProcessAttestationErrors<StateAccessorError<S::Gas>>> {
        let attestation = attestation.inner;
        // We first need to check that the attester is still in the bonding set
        if self
            .bonded_attesters
            .get(context.sender(), state)?
            .is_none()
        {
            return Err(ProcessAttestationErrors::AttesterNotBonded);
        }

        // If the bonding proof in the attestation is invalid, light clients will ignore the attestation. In that case, we should too.
        self.check_bonding_proof(context, &attestation, state)?;

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
            context.sender(),
            &attestation,
            state,
        ) {
            error!(
                error = ?err,
                ?attestation,
                "Error raised when checking initial hashes for attestation");
            return Ok(CallResponse::default());
        }

        // Then compare the transition
        if let Err(err) = self.check_transition(
            attestation.proof_of_bond.claimed_transition_num,
            context.sender(),
            &attestation,
            state,
        ) {
            error!(
                error = ?err,
                ?attestation,
                "Error raised when checking the transition for attestation");
            return Ok(CallResponse::default());
        }

        self.emit_event(
            state,
            Event::<S>::ProcessedValidAttestation {
                attester: context.sender().clone(),
            },
        );

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

            self.transfer_tokens_to_sender(context, self.burn_rate().apply(reward), state)
                .map_err(|err| {
                    error!(
                        error = ?err,
                        "Error raised transferring reward to the attester");
                    ProcessAttestationErrors::RewardTransferFailure
                })?;
        }

        // Then we can optimistically process the transaction
        Ok(CallResponse::default())
    }

    fn check_challenge_outputs_against_transition(
        &self,
        public_outputs: StateTransitionPublicData<S::Address, Da, <S::Storage as Storage>::Root>,
        height: &TransitionHeight,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<(), ProcessChallengeErrors<StateAccessorError<S::Gas>>> {
        let transition = self
            .chain_state
            .get_historical_transitions(*height, state)?
            .ok_or(ProcessChallengeErrors::slashed(
                SlashingReason::TransitionInvalid,
            ))?;

        let initial_hash = {
            if let Some(prev_transition) = self
                .chain_state
                .get_historical_transitions(height.saturating_sub(1), state)?
            {
                prev_transition.post_state_root().clone()
            } else {
                self.chain_state
                    .get_genesis_hash(state)?
                    .expect("The genesis hash should be set")
            }
        };

        if public_outputs.initial_state_root != initial_hash {
            return Err(ProcessChallengeErrors::slashed(
                SlashingReason::InvalidInitialHash,
            ));
        }

        if &public_outputs.slot_hash != transition.slot_hash() {
            return Err(ProcessChallengeErrors::slashed(
                SlashingReason::TransitionInvalid,
            ));
        }

        if public_outputs.validity_condition != *transition.validity_condition() {
            return Err(ProcessChallengeErrors::slashed(
                SlashingReason::TransitionInvalid,
            ));
        }

        Ok(())
    }

    /// Try to process a zk proof if the challenger is bonded.
    /// Same comment as above for the [`AttesterIncentives::process_attestation`] method: if we have a slashable
    /// offense, we want to be able to exit gracefully.
    pub(crate) fn process_challenge(
        &self,
        context: &Context<S>,
        proof: &[u8],
        transition_num: &TransitionHeight,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<CallResponse, ProcessChallengeErrors<StateAccessorError<S::Gas>>> {
        // Get the challenger's old balance.
        // Revert if they aren't bonded
        let old_balance = self
            .bonded_challengers
            .get_or_err(context.sender(), state)?
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
        let attestation_reward: u64 =
            match self.bad_transition_pool.get_or_err(transition_num, state)? {
                Ok(reward) => reward,
                Err(_err) => {
                    self.slash_challenger_burn_reward(
                        context.sender(),
                        SlashingReason::NoInvalidTransition,
                        state,
                    )?;

                    return Ok(CallResponse::default());
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
                    public_output,
                    transition_num,
                    state,
                ) {
                    if let ProcessChallengeErrors::ChallengerSlashed(err) = err {
                        self.slash_challenger_burn_reward(context.sender(), err, state)?;
                        return Ok(CallResponse::default());
                    }

                    return Err(err);
                };

                // Reward the sender
                self.transfer_tokens_to_sender(
                    context,
                    self.burn_rate().apply(attestation_reward),
                    state,
                )
                .map_err(|err| {
                    error!(
                            error = ?err,
                            "Error raised transferring reward to the challenger");
                    ProcessChallengeErrors::RewardTransferFailure
                })?;

                // Now remove the bad transition from the pool
                self.bad_transition_pool.remove(transition_num, state)?;

                self.emit_event(
                    state,
                    Event::<S>::ProcessedValidProof {
                        challenger: context.sender().clone(),
                    },
                );
            }
            Err(_err) => {
                // Slash the challenger
                self.slash_challenger_burn_reward(
                    context.sender(),
                    SlashingReason::InvalidProofOutputs,
                    state,
                )?;
                return Ok(CallResponse::default());
            }
        }

        Ok(CallResponse::default())
    }
}
