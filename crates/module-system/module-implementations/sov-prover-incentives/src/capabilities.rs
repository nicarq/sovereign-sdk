use std::cmp::max;

use sov_modules_api::{
    AggregatedProofPublicData, DaSpec, EventEmitter, Gas, Spec, StateAccessorError, TxState, Zkvm,
};
use thiserror::Error;

use crate::event::SlashingReason;
use crate::{Event, ProverIncentiveError, ProverIncentives};

enum ErrorOrSlashed {
    Error(ProverIncentiveError),
    Slashed(SlashingReason),
}

impl<GU: Gas> From<StateAccessorError<GU>> for ErrorOrSlashed {
    fn from(value: StateAccessorError<GU>) -> Self {
        ErrorOrSlashed::Error(ProverIncentiveError::StateAccessorError(value.to_string()))
    }
}

impl From<SlashingReason> for ErrorOrSlashed {
    fn from(value: SlashingReason) -> Self {
        ErrorOrSlashed::Slashed(value)
    }
}

/// Error raised while processing the aggregated proof.
#[derive(Debug, Error)]
pub enum ProcessProofError<GU: Gas> {
    #[error("The aggregated proof is invalid")]
    InvalidProof,

    #[error("Unable to reward sequencer: {0}")]
    ProverIncentiveError(#[from] ProverIncentiveError),

    #[error("Prover is not bonded at the time of the transaction")]
    ProverNotBonded,

    #[error("The bond is not high enough")]
    BondNotHighEnough,

    #[error("An error occurred when trying to access the state, error: {0}")]
    StateAccessorError(#[from] StateAccessorError<GU>),
}

impl<S: Spec, Da: DaSpec> ProverIncentives<S, Da> {
    /// Try to process a zk proof, if the prover is bonded.
    pub fn process_proof(
        &self,
        proof: &[u8],
        prover_address: &S::Address,
        state: &mut impl TxState<S>,
    ) -> Result<AggregatedProofPublicData, ProcessProofError<S::Gas>> {
        // Get the prover's old balance.
        // Revert if they aren't bonded
        let old_balance = match self.bonded_provers.get(prover_address, state)? {
            Some(balance) => balance,
            None => return Err(ProcessProofError::ProverNotBonded),
        };

        // Check that the prover has enough balance to process the proof.
        let minimum_bond = self.minimum_bond.get(state)?;
        let minimum_bond = minimum_bond.expect("The minimum bond should be set at genesis");

        if old_balance < minimum_bond {
            return Err(ProcessProofError::BondNotHighEnough);
        };
        let new_balance = old_balance.checked_sub(minimum_bond).expect(
            "Underflow happened, while it should've been checked previously. This is a bug.",
        );
        // Lock the prover's bond amount.
        self.bonded_provers
            .set(prover_address, &new_balance, state)?;

        let code_commitment = self
            .chain_state
            .outer_code_commitment(state)?
            .expect("The code commitment should be set at genesis");
        // Don't return an error for invalid proofs - those are expected and shouldn't cause reverts.
        let verification_result =
            <S as Spec>::OuterZkvm::verify::<AggregatedProofPublicData>(proof, &code_commitment);

        let public_outputs = match verification_result {
            Ok(public_outputs) => public_outputs,
            Err(e) => {
                tracing::debug!(verification_error = ?e, "Slashing prover for invalid proof");
                self.emit_event(
                    state,
                    Event::<S>::ProverSlashed {
                        prover: prover_address.clone(),
                        reason: crate::event::SlashingReason::ProofInvalid,
                    },
                );

                return Err(ProcessProofError::InvalidProof);
            }
        };

        // Check that the public outputs are valid
        if let Err(err) = self.check_proof_outputs(&public_outputs, state) {
            match err {
                ErrorOrSlashed::Error(err) => return Err(err.into()),
                ErrorOrSlashed::Slashed(reason) => {
                    tracing::debug!(?reason, "Slashing prover");
                    self.emit_event(
                        state,
                        Event::<S>::ProverSlashed {
                            prover: prover_address.clone(),
                            reason,
                        },
                    );
                    return Err(ProcessProofError::InvalidProof);
                }
            }
        }

        // Let's check the initial and final state values
        let new_staked_balance = self.try_reward_prover(
            public_outputs.initial_slot_number,
            public_outputs.final_slot_number,
            old_balance,
            prover_address,
            state,
        )?;

        // Unlock the prover's bond
        self.bonded_provers
            .set(prover_address, &new_staked_balance, state)?;

        Ok(public_outputs)
    }

    /// Computes the total reward from the aggregated state transition and rewards the prover with the unclaimed
    /// transition rewards. If all the rewards were already claimed, the prover is fined by a constant amount.
    fn try_reward_prover(
        &self,
        init_slot_num: u64,
        final_slot_num: u64,
        old_balance: u64,
        sender: &S::Address,
        state: &mut impl TxState<S>,
    ) -> Result<u64, ProverIncentiveError> {
        // Let's compute the total reward
        let mut total_reward = 0;

        let first_available_reward = self
            .last_claimed_reward
            .get(state)?
            .expect("The last claimed reward should be set at genesis")
            + 1;

        // The first reward we can claim is the maximum between the initial slot number and the first available reward
        let first_claimed_reward = max(init_slot_num, first_available_reward);

        // Here the final slot number is inclusive
        for slot_num in first_claimed_reward..=final_slot_num {
            // Check if the reward was already claimed

            // If not, reward the prover with the block reward
            // `get_historical_transitions` should always return `Some` because we are iterating over the range of `init_slot_num..=final_slot_num`
            // whose integrity was checked beforehand.
            if let Some(transition) = self
                .chain_state
                .get_historical_transitions(slot_num, state)?
            {
                let curr_reward = transition.gas_used().value(transition.gas_price());
                total_reward += curr_reward;
            }
        }

        // We need to remove the reward once it is claimed
        self.last_claimed_reward
            .set(&max(first_available_reward, final_slot_num), state)?;

        if first_claimed_reward > final_slot_num {
            // We need to fine the prover
            let fine = self
                .proving_penalty
                .get(state)?
                .expect("Should be set at genesis");

            // Unlock the prover's bond
            self.bonded_provers
                .set(sender, &(old_balance - fine), state)?;

            self.emit_event(
                state,
                Event::<S>::ProverPenalized {
                    prover: sender.clone(),
                    amount: fine,
                    reason: crate::event::PenalizationReason::ProofAlreadyProcessed,
                },
            );

            return Ok(old_balance - fine);
        }

        // We only reward a portion of the total reward - we burn some of it
        // to avoid the provers to collude to prove empty blocks.
        let reward_amount = self.burn_rate().apply(total_reward);
        self.transfer_to_prover(reward_amount, sender, state)?;

        self.emit_event(
            state,
            Event::<S>::ProcessedValidProof {
                prover: sender.clone(),
                reward: reward_amount,
            },
        );

        Ok(old_balance)
    }

    /// Check that the initial and final state values of the proof output are valid against the chain state module
    fn check_proof_outputs(
        &self,
        public_outputs: &AggregatedProofPublicData,
        state: &mut impl TxState<S>,
    ) -> Result<(), ErrorOrSlashed> {
        let expected_genesis_hash = self
            .chain_state
            .get_genesis_hash(state)?
            .expect("The genesis hash should be set at genesis");

        // We have to check that the genesis hash is valid
        if expected_genesis_hash.as_ref() != public_outputs.genesis_state_root {
            return Err(SlashingReason::IncorrectGenesisHash.into());
        }

        // We start with the initial state values
        let initial_slot_num = public_outputs.initial_slot_number;

        let initial_transition = self
            .chain_state
            .get_historical_transitions(initial_slot_num, state)?
            .ok_or(SlashingReason::InitialTransitionDoesNotExist)?;

        let initial_state_root = if let Some(prev_transition) = self
            .chain_state
            .get_historical_transitions(initial_slot_num.saturating_sub(1), state)?
        {
            prev_transition.post_state_root().clone()
        } else {
            expected_genesis_hash
        };

        if initial_state_root.as_ref() != public_outputs.initial_state_root {
            return Err(SlashingReason::IncorrectInitialStateRoot.into());
        }

        let initial_transition_hash = initial_transition.slot_hash();

        if initial_transition_hash.as_ref() != public_outputs.initial_slot_hash {
            return Err(SlashingReason::IncorrectInitialSlotHash.into());
        }

        // Let's move on to the final state values
        let final_slot_num = public_outputs.final_slot_number;
        let expected_final_transition = self
            .chain_state
            .get_historical_transitions(final_slot_num, state)?
            .ok_or(SlashingReason::FinalTransitionDoesNotExist)?;

        if expected_final_transition.post_state_root().as_ref() != public_outputs.final_state_root {
            return Err(SlashingReason::IncorrectFinalStateRoot.into());
        }

        if expected_final_transition.slot_hash().as_ref() != public_outputs.final_slot_hash {
            return Err(SlashingReason::IncorrectFinalSlotHash.into());
        }

        // We may also want to check the integrity of the validity conditions along the way
        // We first need to check the length of the validity conditions vector
        if public_outputs.validity_conditions.len()
            != (final_slot_num - initial_slot_num + 1) as usize
        {
            return Err(SlashingReason::IncorrectValidityConditions.into());
        }

        // We are checking all the validity conditions up to `final_slot_num` included.
        for (slot_num, output_condition) in
            (initial_slot_num..=final_slot_num).zip(public_outputs.validity_conditions.iter())
        {
            match self
                .chain_state
                .get_historical_transitions(slot_num, state)?
            {
                Some(transition) => {
                    if borsh::to_vec(transition.validity_condition())
                        .expect("Should always be able to serialize the validity condition")
                        != output_condition.clone()
                    {
                        return Err(SlashingReason::IncorrectValidityConditions.into());
                    }
                }
                None => return Err(SlashingReason::IncorrectValidityConditions.into()),
            }
        }

        Ok(())
    }
}
