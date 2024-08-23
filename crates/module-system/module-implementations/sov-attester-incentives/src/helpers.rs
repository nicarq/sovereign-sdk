use anyhow::ensure;
use borsh::BorshDeserialize;
use sov_bank::{BurnRate, Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::hooks::TransitionHeight;
use sov_modules_api::macros::config_value;
use sov_modules_api::optimistic::Attestation;
use sov_modules_api::{
    EventEmitter, StateAccessor, StateAccessorError, StateTransitionPublicData, StateWriter,
    TxState,
};
use sov_state::storage::{SlotKey, SlotValue, Storage, StorageProof};
use sov_state::User;
use tracing::{debug, error};

use crate::{
    AttesterIncentives, Event, ProcessAttestationErrors, ProcessChallengeErrors, SlashingReason,
};

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
    ) -> anyhow::Result<Option<SlotValue>> {
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

    #[allow(clippy::type_complexity)]
    pub(crate) fn check_initial_hash(
        &self,
        claimed_transition_height: TransitionHeight,
        attester: &S::Address,
        attestation: &Attestation<
            Da::SlotHash,
            <S::Storage as Storage>::Root,
            StorageProof<<S::Storage as Storage>::Proof>,
        >,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<(), ProcessAttestationErrors<StateAccessorError<S::Gas>>> {
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
                return Ok(());
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

                return Ok(());
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

                return Ok(());
            }
            // Normal state
        }

        Ok(())
    }

    /// A helper function that simply slashes an attester and returns a reward value
    pub(crate) fn slash_attester<TxStateAccessor: TxState<S>>(
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

    /// A helper function that is used to slash an attester, and put the associated attestation in the slashed pool
    pub(crate) fn slash_and_invalidate_attestation<TxStateAccessor: TxState<S>>(
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

    pub(crate) fn slash_attester_burn_reward(
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

    pub(crate) fn slash_challenger_burn_reward(
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

    pub(crate) fn transfer_tokens_to_sender(
        &self,
        sender: &S::Address,
        amount: u64,
        state: &mut impl StateAccessor,
    ) -> anyhow::Result<()> {
        let coins = Coins {
            token_id: GAS_TOKEN_ID,
            amount,
        };

        // The reward tokens are unlocked from the module's id.
        self.bank
            .transfer_from(self.id.to_payable(), sender, coins, state)?;

        Ok(())
    }

    /// The bonding proof is now a proof that an attester was bonded during the last `finality_period` range.
    /// The proof must refer to a valid state of the rollup. The initial root hash must represent a state between
    /// the bonding proof one and the current state.
    #[allow(clippy::type_complexity)]
    pub(crate) fn check_bonding_proof(
        &self,
        sender: &S::Address,
        attestation: &Attestation<
            Da::SlotHash,
            <S::Storage as Storage>::Root,
            StorageProof<<S::Storage as Storage>::Proof>,
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
                sender,
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
    pub(crate) fn check_transition(
        &self,
        claimed_transition_height: TransitionHeight,
        attester: &S::Address,
        attestation: &Attestation<
            Da::SlotHash,
            <S::Storage as Storage>::Root,
            StorageProof<<S::Storage as Storage>::Proof>,
        >,
        state: &mut impl TxState<S>,
    ) -> Result<(), ProcessAttestationErrors<StateAccessorError<S::Gas>>> {
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
        } else {
            // Case where we cannot get the transition from the chain state historical transitions.
            self.slash_attester_burn_reward(attester, SlashingReason::TransitionNotFound, state)?;
        }
        Ok(())
    }

    pub(crate) fn check_challenge_outputs_against_transition(
        &self,
        public_outputs: &StateTransitionPublicData<S::Address, Da, <S::Storage as Storage>::Root>,
        height: TransitionHeight,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<(), ProcessChallengeErrors<StateAccessorError<S::Gas>>> {
        let transition = self
            .chain_state
            .get_historical_transitions(height, state)?
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
}
