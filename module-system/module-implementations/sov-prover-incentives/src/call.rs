use std::cmp::max;
use std::fmt::Debug;

use anyhow::{Context as AnyhowContext, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_bank::{BurnRate, Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::macros::config_constant;
use sov_modules_api::{
    AggregatedProofPublicData, CallResponse, Context, DaSpec, EventEmitter, Gas, Spec, WorkingSet,
    Zkvm,
};
use thiserror::Error;

use crate::event::SlashingReason;
use crate::{Event, ProverIncentives};

/// This enumeration represents the available call messages for interacting with the `ExampleModule` module.
#[cfg_attr(feature = "native", derive(schemars::JsonSchema))]
#[derive(Serialize, Deserialize, BorshDeserialize, BorshSerialize, Debug, PartialEq)]
// TODO: allow call messages to borrow data
//     https://github.com/Sovereign-Labs/sovereign-sdk/issues/274
pub enum CallMessage {
    /// Bonds the prover with provided bond.
    BondProver(u64),
    /// Unbonds the prover.
    UnbondProver,
    /// Verifies the provided proof (of format `Vec<u8>`)
    VerifyProof(Vec<u8>),
}

/// Error raised while processing the attester incentives
#[derive(Debug, Error, PartialEq)]
pub enum ProverIncentiveError {
    #[error("The bond is not high enough")]
    /// The bond is below the minimum bond
    BondNotHighEnough,

    #[error("Prover is not bonded at the time of the transaction")]
    /// User is not bonded at the time of the transaction
    ProverNotBonded,

    #[error("Error occurred when transferring funds to bond the prover. The prover's account may not have enough funds")]
    /// An error occurred when transferring funds to bond the prover
    BondTransferFailure,

    #[error("Error occurred when transferring funds to unbond or reward the prover. This module's account may not have enough funds.
    This is a bug")]
    /// An error occurred when trying to mint the reward token
    TransferFailure,

    /// An error when total bond value overflow or underflow
    #[error("Error when trying to top up bonded amount and it overflow or underflow")]
    BondArithmeticsError,
}

impl<S: Spec, Da: DaSpec> ProverIncentives<S, Da> {
    /// The burn rate of the reward price for the provers.
    /// The burn rate is a percentage of the base fee that is burned - this prevents provers from proving empty blocks.
    pub(crate) const fn burn_rate(&self) -> BurnRate {
        #[config_constant]
        const PERCENT_BASE_FEE_TO_BURN: u8;

        BurnRate::new_unchecked(PERCENT_BASE_FEE_TO_BURN)
    }
    /// A helper function for the `bond_prover` call. Also used to bond provers
    /// during genesis when no context is available.
    pub(super) fn bond_prover_helper(
        &self,
        bond_amount: u64,
        prover: &S::Address,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse, ProverIncentiveError> {
        // Transfer the bond amount from the sender to the module's id.
        // On failure, no state is changed
        let coins = Coins {
            token_id: GAS_TOKEN_ID,
            amount: bond_amount,
        };
        self.bank
            .transfer_from(prover, self.id.to_payable(), coins, working_set)
            .map_err(|_| ProverIncentiveError::BondTransferFailure)?;

        // Check that total balance does not overflow before doing transfer.
        let old_balance = self
            .bonded_provers
            .get(prover, working_set)
            .unwrap_or_default();

        let total_balance = old_balance
            .checked_add(bond_amount)
            .with_context(|| {
                anyhow::anyhow!("The total balance overflows with the given operation")
            })
            .map_err(|_e| ProverIncentiveError::BondArithmeticsError)?;

        // Update our record of the total bonded amount for the sender.
        // This update is infallible, so no value can be destroyed.
        self.bonded_provers.set(prover, &total_balance, working_set);

        // Emit the bonding event
        self.emit_event(
            working_set,
            "bond_prover_helper",
            Event::<S>::BondedProver {
                prover: prover.clone(),
                deposit: bond_amount,
                total_balance,
            },
        );

        Ok(CallResponse::default())
    }

    /// Try to bond the requested amount of coins from context.sender()
    pub(crate) fn bond_prover(
        &self,
        bond_amount: u64,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse, ProverIncentiveError> {
        self.bond_prover_helper(bond_amount, context.sender(), working_set)
    }

    /// Try to unbond the requested amount of coins with context.sender() as the beneficiary.
    pub(crate) fn unbond_prover(
        &self,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse, ProverIncentiveError> {
        // Get the prover's old balance.
        if let Some(old_balance) = self.bonded_provers.get(context.sender(), working_set) {
            self.transfer_to_prover(old_balance, context, working_set)?;

            // Update our internal tracking of the total bonded amount for the sender.
            self.bonded_provers.set(context.sender(), &0, working_set);

            // Emit the unbonding event
            self.emit_event(
                working_set,
                "unbond_prover",
                Event::<S>::UnBondedProver {
                    prover: context.sender().clone(),
                    amount_withdrawn: old_balance,
                },
            );
        }

        Ok(CallResponse::default())
    }

    /// Check that the initial and final state values of the proof output are valid against the chain state module
    fn check_proof_outputs(
        &self,
        public_outputs: &AggregatedProofPublicData,
        working_set: &mut WorkingSet<S>,
    ) -> Result<(), SlashingReason> {
        let expected_genesis_hash = self
            .chain_state
            .get_genesis_hash(working_set)
            .expect("The genesis hash should be set at genesis");

        // We have to check that the genesis hash is valid
        if expected_genesis_hash.as_ref() != public_outputs.genesis_state_root {
            return Err(SlashingReason::IncorrectGenesisHash);
        }

        // We start with the initial state values
        let initial_slot_num = public_outputs.initial_slot_number;

        let initial_transition = self
            .chain_state
            .get_historical_transitions(initial_slot_num, working_set)
            .ok_or(SlashingReason::InitialTransitionDoesNotExist)?;

        let initial_state_root = if let Some(prev_transition) = self
            .chain_state
            .get_historical_transitions(initial_slot_num.saturating_sub(1), working_set)
        {
            prev_transition.post_state_root().clone()
        } else {
            expected_genesis_hash
        };

        if initial_state_root.as_ref() != public_outputs.initial_state_root {
            return Err(SlashingReason::IncorrectInitialStateRoot);
        }

        let initial_transition_hash = initial_transition.slot_hash();

        if initial_transition_hash.as_ref() != public_outputs.initial_slot_hash {
            return Err(SlashingReason::IncorrectInitialSlotHash);
        }

        // Let's move on to the final state values
        let final_slot_num = public_outputs.final_slot_number;
        let expected_final_transition = self
            .chain_state
            .get_historical_transitions(final_slot_num, working_set)
            .ok_or(SlashingReason::FinalTransitionDoesNotExist)?;

        if expected_final_transition.post_state_root().as_ref() != public_outputs.final_state_root {
            return Err(SlashingReason::IncorrectFinalStateRoot);
        }

        if expected_final_transition.slot_hash().as_ref() != public_outputs.final_slot_hash {
            return Err(SlashingReason::IncorrectFinalSlotHash);
        }

        // We may also want to check the integrity of the validity conditions along the way
        // We first need to check the length of the validity conditions vector
        if public_outputs.validity_conditions.len()
            != (final_slot_num - initial_slot_num + 1) as usize
        {
            return Err(SlashingReason::IncorrectValidityConditions);
        }

        // We are checking all the validity conditions up to `final_slot_num` included.
        for (slot_num, output_condition) in
            (initial_slot_num..=final_slot_num).zip(public_outputs.validity_conditions.iter())
        {
            match self
                .chain_state
                .get_historical_transitions(slot_num, working_set)
            {
                Some(transition) => {
                    if transition
                        .validity_condition()
                        .try_to_vec()
                        .expect("Should always be able to serialize the validity condition")
                        != output_condition.clone()
                    {
                        return Err(SlashingReason::IncorrectValidityConditions);
                    }
                }
                None => return Err(SlashingReason::IncorrectValidityConditions),
            }
        }

        Ok(())
    }

    /// Transfer the given amount of tokens to the prover
    fn transfer_to_prover(
        &self,
        total_reward: u64,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<(), ProverIncentiveError> {
        let coins = Coins {
            token_id: GAS_TOKEN_ID,
            amount: total_reward,
        };

        // We can transfer the reward from the `ProverIncentives` module to the prover's account.
        self.bank
            .transfer_from(self.id.to_payable(), context.sender(), coins, working_set)
            .map_err(|_| ProverIncentiveError::TransferFailure)?;

        Ok(())
    }

    /// Computes the total reward from the aggregated state transition and rewards the prover with the unclaimed
    /// transition rewards. If all the rewards were already claimed, the prover is fined by a constant amount.
    fn try_reward_prover(
        &self,
        init_slot_num: u64,
        final_slot_num: u64,
        old_balance: u64,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<u64, ProverIncentiveError> {
        // Let's compute the total reward
        let mut total_reward = 0;

        let first_available_reward = self
            .last_claimed_reward
            .get(working_set)
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
                .get_historical_transitions(slot_num, working_set)
            {
                let curr_reward = transition.gas_used().value(transition.gas_price());
                total_reward += curr_reward;
            }
        }

        // We need to remove the reward once it is claimed
        self.last_claimed_reward
            .set(&max(first_available_reward, final_slot_num), working_set);

        if total_reward > 0 {
            // We only reward a portion of the total reward - we burn some of it
            // to avoid the provers to collude to prove empty blocks.
            let reward_amount = self.burn_rate().apply(total_reward);
            self.transfer_to_prover(reward_amount, context, working_set)?;

            self.emit_event(
                working_set,
                "process_valid_proof",
                Event::<S>::ProcessedValidProof {
                    prover: context.sender().clone(),
                    reward: reward_amount,
                },
            );

            Ok(old_balance)
        } else {
            // We need to fine the prover
            let fine = self
                .proving_penalty
                .get(working_set)
                .expect("Should be set at genesis");

            // Unlock the prover's bond
            self.bonded_provers
                .set(context.sender(), &(old_balance - fine), working_set);

            self.emit_event(
                working_set,
                "prover_penalized",
                Event::<S>::ProverPenalized {
                    prover: context.sender().clone(),
                    amount: fine,
                    reason: crate::event::PenalizationReason::ProofAlreadyProcessed,
                },
            );

            Ok(old_balance - fine)
        }
    }

    /// Try to process a zk proof, if the prover is bonded.
    pub(crate) fn process_proof(
        &self,
        proof: &[u8],
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse, ProverIncentiveError> {
        // Get the prover's old balance.
        // Revert if they aren't bonded
        let old_balance = match self.bonded_provers.get(context.sender(), working_set) {
            Some(balance) => balance,
            None => return Err(ProverIncentiveError::ProverNotBonded),
        };

        // Check that the prover has enough balance to process the proof.
        let minimum_bond = self
            .minimum_bond
            .get(working_set)
            .expect("The minimum bond should be set at genesis");

        if old_balance < minimum_bond {
            return Err(ProverIncentiveError::BondNotHighEnough);
        };
        let new_balance = old_balance.checked_sub(minimum_bond).expect(
            "Underflow happened, while it should've been checked previously. This is a bug.",
        );
        // Lock the prover's bond amount.
        self.bonded_provers
            .set(context.sender(), &new_balance, working_set);

        let code_commitment = self
            .chain_state
            .outer_code_commitment(working_set)
            .expect("The code commitment should be set at genesis");
        // Don't return an error for invalid proofs - those are expected and shouldn't cause reverts.
        let verification_result =
            <S as Spec>::OuterZkvm::verify::<AggregatedProofPublicData>(proof, &code_commitment);

        let public_outputs = match verification_result {
            Ok(public_outputs) => public_outputs,
            Err(_) => {
                self.emit_event(
                    working_set,
                    "prover_slashed",
                    Event::<S>::ProverSlashed {
                        prover: context.sender().clone(),
                        reason: crate::event::SlashingReason::ProofInvalid,
                    },
                );

                return Ok(CallResponse::default());
            }
        };

        // Check that the public outputs are valid
        if let Err(err) = self.check_proof_outputs(&public_outputs, working_set) {
            self.emit_event(
                working_set,
                "prover_slashed",
                Event::<S>::ProverSlashed {
                    prover: context.sender().clone(),
                    reason: err,
                },
            );

            return Ok(CallResponse::default());
        }

        // Let's check the initial and final state values
        let new_staked_balance = self.try_reward_prover(
            public_outputs.initial_slot_number,
            public_outputs.final_slot_number,
            old_balance,
            context,
            working_set,
        )?;

        // Unlock the prover's bond
        self.bonded_provers
            .set(context.sender(), &new_staked_balance, working_set);

        Ok(CallResponse::default())
    }
}
