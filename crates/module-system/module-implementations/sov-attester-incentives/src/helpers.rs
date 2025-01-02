use anyhow::ensure;
use borsh::BorshDeserialize;
use sov_bank::{config_gas_token_id, BurnRate, Coins, IntoPayable};
use sov_modules_api::macros::config_value;
use sov_modules_api::optimistic::Attestation;
use sov_modules_api::{
    DaSpec, Gas, Spec, StateAccessor, StateTransitionPublicData, TxState, VersionReader,
};
use sov_rollup_interface::common::SlotNumber;
use sov_state::storage::{SlotKey, SlotValue, Storage, StorageProof};
use tracing::debug;

use crate::{AttesterIncentives, ProcessAttestationErrors, SlashingReason};

impl<S> AttesterIncentives<S>
where
    S: Spec,
{
    /// Returns the burn rate for the reward
    pub fn burn_rate(&self) -> BurnRate {
        BurnRate::new_unchecked(config_value!("PERCENT_BASE_FEE_TO_BURN"))
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
    pub(crate) fn check_initial_hash<ST: VersionReader>(
        &self,
        claimed_rollup_height: SlotNumber,
        attestation: &Attestation<
            <S::Da as DaSpec>::SlotHash,
            <S::Storage as Storage>::Root,
            StorageProof<<S::Storage as Storage>::Proof>,
        >,
        state: &mut ST,
    ) -> anyhow::Result<CheckInitialHashStatus, ST::Error> {
        let previous_height = claimed_rollup_height
            .checked_sub(1)
            .expect("Claimed transition height must be > 0");

        // Normal state
        let Some(claimed_root) = self.chain_state.root_at_height(previous_height, state)? else {
            return Ok(CheckInitialHashStatus::Slash);
        };

        if claimed_root != attestation.initial_state_root {
            // The initial root hashes don't match, just slash the attester
            return Ok(CheckInitialHashStatus::Slash);
        }

        Ok(CheckInitialHashStatus::Valid)
    }

    /// A helper function that simply slashes an attester and returns a reward value
    pub(crate) fn slash_attester<TxStateAccessor: TxState<S>>(
        &self,
        user: &S::Address,
        state: &mut TxStateAccessor,
    ) -> Result<u64, anyhow::Error> {
        // We have to remove the attester from the unbonding set
        // to prevent him from skipping the first phase
        // unbonding if he bonds himself again.
        self.unbonding_attesters.remove(user, state)?;
        let bonded_set = &self.bonded_attesters;

        // We have to deplete the attester's bonded account, it amounts to removing the attester from the bonded set
        let reward = bonded_set.get(user, state)?.unwrap_or_default();
        bonded_set.remove(user, state)?;

        Ok(reward)
    }

    /// A helper function that is used to slash an attester, and put the associated attestation in the slashed pool
    pub(crate) fn slash_and_invalidate_attestation<TxStateAccessor: TxState<S>>(
        &self,
        attester: &S::Address,
        height: SlotNumber,
        state: &mut TxStateAccessor,
    ) -> Result<(), anyhow::Error> {
        let reward = self.slash_attester(attester, state)?;

        let curr_reward_value = self
            .bad_transition_pool
            .get(&height, state)?
            .unwrap_or_default();

        let new_value = curr_reward_value.saturating_add(reward);
        self.bad_transition_pool.set(&height, &new_value, state)?;

        Ok(())
    }

    pub(crate) fn transfer_tokens_to_sender(
        &self,
        sender: &S::Address,
        amount: u64,
        state: &mut impl StateAccessor,
    ) -> anyhow::Result<()> {
        let coins = Coins {
            token_id: config_gas_token_id(),
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
    pub(crate) fn check_bonding_proof<ST: TxState<S>>(
        &self,
        sender: &S::Address,
        attestation: &Attestation<
            <S::Da as DaSpec>::SlotHash,
            <S::Storage as Storage>::Root,
            StorageProof<<S::Storage as Storage>::Proof>,
        >,
        state: &mut ST,
    ) -> Result<(), ProcessAttestationErrors> {
        if attestation.proof_of_bond.claimed_rollup_height == SlotNumber::GENESIS {
            debug!("Cannot claim attestation for genesis");
            return Err(ProcessAttestationErrors::InvalidTransitionInvariant);
        }

        // If we cannot get the transition before the current one, it means that we are trying
        // to get the genesis state root
        let Some(bonding_root) = self
            .chain_state
            .root_at_height(attestation.proof_of_bond.claimed_rollup_height, state)
            .map_err(Into::<anyhow::Error>::into)?
        else {
            return Err(ProcessAttestationErrors::InvalidBondingProof);
        };

        // This proof checks that the attester was bonded at the given transition num
        let bond_opt = self
            .verify_proof(
                bonding_root,
                attestation.proof_of_bond.proof.clone(),
                sender,
            )
            .map_err(|err| {
                debug!(error = ?err, "Error during verifying bonding proof");
                ProcessAttestationErrors::InvalidBondingProof
            })?;

        let bond = bond_opt.ok_or(ProcessAttestationErrors::AttesterNotBonded)?;
        let bond: u64 = BorshDeserialize::deserialize(&mut bond.value())
            .map_err(|_err| ProcessAttestationErrors::InvalidBondFormat)?;

        let minimum_bond = self
            .minimum_attester_bond
            .get_or_err(state)
            .map_err(Into::<anyhow::Error>::into)?
            .expect("The minimum bond should be set at genesis");

        // We then have to check that the bond was greater than the minimum bond
        if bond < minimum_bond.value(&state.gas_info().gas_price) {
            return Err(ProcessAttestationErrors::AttesterNotBonded);
        }

        Ok(())
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn check_transition<ST: VersionReader>(
        &self,
        claimed_rollup_height: SlotNumber,
        attestation: &Attestation<
            <S::Da as DaSpec>::SlotHash,
            <S::Storage as Storage>::Root,
            StorageProof<<S::Storage as Storage>::Proof>,
        >,
        state: &mut ST,
    ) -> Result<CheckTransitionStatus, ST::Error> {
        if let Some(curr_tx) = self
            .chain_state
            .get_historical_transitions(claimed_rollup_height, state)?
        {
            // We first need to compare the initial block hash to the previous post state root
            if !curr_tx.compare_hashes(&attestation.slot_hash, &attestation.post_state_root) {
                debug!(
                    %claimed_rollup_height,
                    attestation_slot_hash = ?attestation.slot_hash,
                    attestation_post_state = ?attestation.post_state_root,
                    curr_tx_slot_hash = ?curr_tx.slot_hash(),
                    curr_tx_state_root = ?curr_tx.post_state_root(),
                    "The attestation has an invalid block hash or post state root");
                // Check if the attestation has the same slot_hash and post_state_root as the actual transition
                // that we found in state. If not, slash the attester.
                // If so, the attestation is valid, so return Ok
                return Ok(CheckTransitionStatus::SlashInvalidateWrongHash);
            }
        } else {
            // Case where we cannot get the transition from the chain state historical transitions.
            return Ok(CheckTransitionStatus::SlashedNoHistoricalTransition);
        }

        Ok(CheckTransitionStatus::Valid)
    }

    pub(crate) fn check_challenge_outputs_against_transition<ST: VersionReader>(
        &self,
        public_outputs: &StateTransitionPublicData<
            S::Address,
            S::Da,
            <S::Storage as Storage>::Root,
        >,
        height: SlotNumber,
        state: &mut ST,
    ) -> anyhow::Result<Option<SlashingReason>, ST::Error> {
        let transition = match self.chain_state.get_historical_transitions(height, state)? {
            Some(transition) => transition,
            None => return Ok(Some(SlashingReason::TransitionInvalid)),
        };

        let Some(initial_hash) = self
            .chain_state
            .root_at_height(height.saturating_sub(1), state)?
        else {
            return Ok(Some(SlashingReason::TransitionInvalid));
        };

        if public_outputs.initial_state_root != initial_hash {
            return Ok(Some(SlashingReason::InvalidInitialHash));
        }

        if &public_outputs.slot_hash != transition.slot_hash() {
            return Ok(Some(SlashingReason::TransitionInvalid));
        }

        if public_outputs.validity_condition != *transition.validity_condition() {
            return Ok(Some(SlashingReason::TransitionInvalid));
        }

        Ok(None)
    }

    pub(crate) fn slash_challenger(
        &self,
        sender: &S::Address,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<(), anyhow::Error> {
        self.bonded_challengers.remove(sender, state)?;
        Ok(())
    }
}

pub(crate) enum CheckInitialHashStatus {
    Valid,
    Slash,
}

pub(crate) enum CheckTransitionStatus {
    Valid,
    SlashedNoHistoricalTransition,
    SlashInvalidateWrongHash,
}
