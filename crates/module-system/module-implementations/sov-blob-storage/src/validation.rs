use sov_bank::derived_holder::DerivedHolder;
use sov_bank::IntoPayable;
use sov_modules_api::digest::Digest;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{
    BatchWithId, BlobDataWithId, CryptoSpec, DaSpec, Gas, GasArray, GasSpec, KernelStateAccessor,
    ModuleInfo, PrivilegedKernelAccessor, Spec,
};

use crate::max_size_checker::BlobsWithTotalSizeLimit;
use crate::{BlobStorage, Escrow, ValidatedBlob};

impl<S: Spec> BlobStorage<S> {
    pub(crate) fn get_new_gas_price(
        &self,
        visible_height_increase: u64,
        state: &mut KernelStateAccessor<'_, S>,
    ) -> <<S as Spec>::Gas as Gas>::Price {
        // This is the rollup height of the previous block, because blob storage executes before height increments.
        let stale_rollup_height = self.chain_state.rollup_height(state).unwrap_infallible();
        self.chain_state
            .compute_next_gas_price(stale_rollup_height, visible_height_increase, state)
            .unwrap_infallible()
    }

    /// This function determines whether a blob should be accepted by making an informed guess about how much gas the sequencer will need to pay for the blob,
    /// and setting aside enough balance to meet that cost. This function is just a first line of defense. If it incorrectly accepts a blob,
    /// this is *not* a significant vulnerability - all that will happen is that the sequencer will pay to have the blob stored, and then it will be rejected
    /// later, resulting in some wasted gas.
    ///
    /// The crucial check that must be correct is ensuring that the sequencer has enough balance to pay for the gas needed to run pre-execution checks *at the time the blob is selected* for execution.
    /// That check can be done with no guesswork.
    pub(crate) fn validate_blob(
        &self,
        idx: u64,
        blob: BlobDataWithId<S, BatchWithId<S>>,
        sender: <<S as Spec>::Da as DaSpec>::Address,
        available_balance: u64,
        selected_blobs: &BlobsWithTotalSizeLimit<S>,
        account_for_deferral: bool,
        visible_height_increase: u64,
        state: &mut KernelStateAccessor<'_, S>,
    ) -> Option<ValidatedBlob<S, BatchWithId<S>>> {
        if !selected_blobs.can_accept_blob(blob.blob_size()) {
            return None;
        }

        let mut funds_needed = 0u64;
        let best_gas_price_estimate = self.get_new_gas_price(visible_height_increase, state);

        // If we might defer this blob, we need to account for storage costs and account for the fact that the gas price might be higher.
        // when it gets selected for execution.
        if account_for_deferral {
            let tokens_needed_for_deferral =
                self.account_for_deferral_costs(&blob, &sender, &best_gas_price_estimate)?;
            funds_needed = funds_needed.checked_add(tokens_needed_for_deferral)?;
        }

        // We need to run 1 pre-flight check for each tx in the batch, or 1 for the proof
        let num_pre_exec_checks_needed = if let BlobDataWithId::Batch(batch) = &blob {
            batch.batch.len()
        } else {
            1
        };
        let gas_needed_for_pre_exec_checks = <S as GasSpec>::max_tx_check_costs()
            .checked_scalar_product(num_pre_exec_checks_needed as u64)?;
        funds_needed = funds_needed
            .checked_add(gas_needed_for_pre_exec_checks.checked_value(&best_gas_price_estimate)?)?;
        if funds_needed > available_balance {
            tracing::debug!(funds_needed, %sender, available_balance, "Failed to escrow funds for deferred blob.");
            return None;
        }

        // If the blobs are being deferred, we store the balance per sequencer in separate derived accounts.
        if account_for_deferral {
            let new_holder: DerivedHolder = self.compute_derived_holder(&blob, idx, state);
            if let Err(e) = self.sequencer_registry.remove_part_of_the_stake(
                &sender,
                new_holder.to_payable(),
                funds_needed,
                state,
            ) {
                tracing::debug!(funds_needed, %sender, "Failed to escrow funds for deferred blob. {}", e);
                return None;
            };
            Some(ValidatedBlob::new(
                blob,
                sender,
                Escrow::DerivedHolder(new_holder),
            ))
        } else {
            // Otherwise, we just transfer the balance to the bank. We'll refund the sequencer's balance directly from there
            if let Err(e) = self.sequencer_registry.remove_part_of_the_stake(
                &sender,
                self.bank.id().to_payable(),
                funds_needed,
                state,
            ) {
                tracing::debug!(funds_needed, %sender, "Failed to escrow funds. {}", e);
                return None;
            }
            Some(ValidatedBlob::new(
                blob,
                sender,
                Escrow::Direct(funds_needed),
            ))
        }
    }

    /// Validate the preferred blob and reserve funds for the pre-exec checks.
    /// Note that for preferred blobs, we only reserve funds for a single transaction. This is because
    /// the preferred sequencer doesn't know in advance how many transactions it will submit in a batch.
    pub(crate) fn validate_preferred_blob(
        &self,
        blob: BlobDataWithId<S, BatchWithId<S>>,
        sender: <<S as Spec>::Da as DaSpec>::Address,
        available_balance: u64,
        selected_blobs: &BlobsWithTotalSizeLimit<S>,
        visible_height_increase: u64,
        state: &mut KernelStateAccessor<'_, S>,
    ) -> Option<ValidatedBlob<S, BatchWithId<S>>> {
        if !selected_blobs.can_accept_blob(blob.blob_size()) {
            return None;
        }
        let best_gas_price_estimate = self.get_new_gas_price(visible_height_increase, state);

        let gas_needed_for_pre_exec_checks = <S as GasSpec>::max_tx_check_costs();
        let funds_needed =
            gas_needed_for_pre_exec_checks.checked_value(&best_gas_price_estimate)?;
        if funds_needed > available_balance {
            return None;
        }

        self.escrow_funds_for_preferred_sequencer(funds_needed, state)
            .ok()?;
        Some(ValidatedBlob::new(
            blob,
            sender,
            Escrow::Direct(funds_needed),
        ))
    }

    fn compute_derived_holder(
        &self,
        blob: &BlobDataWithId<S, BatchWithId<S>>,
        idx: u64,
        state: &mut KernelStateAccessor<'_, S>,
    ) -> DerivedHolder {
        let mut hasher = <S::CryptoSpec as CryptoSpec>::Hasher::new();
        hasher.update(blob.id());
        hasher.update(idx.to_le_bytes());
        hasher.update(state.true_slot_number().get().to_le_bytes());
        let hash: [u8; 32] = hasher.finalize().into();
        DerivedHolder::from(hash)
    }

    pub(crate) fn num_pre_exec_checks_needed(blob: &BlobDataWithId<S, BatchWithId<S>>) -> u64 {
        if let BlobDataWithId::Batch(batch) = blob {
            batch.batch.len() as u64
        } else {
            1
        }
    }

    /// This function is used to account for the costs of deferring a blob. It covers the cost of storing and retrieving the blob, as well as the potential increase
    /// in the gas needed to run pre-execution checks.
    fn account_for_deferral_costs(
        &self,
        blob: &BlobDataWithId<S, BatchWithId<S>>,
        sender: &<<S as Spec>::Da as DaSpec>::Address,
        current_gas_price: &<<S as Spec>::Gas as Gas>::Price,
    ) -> Option<u64> {
        const WORST_CASE_GAS_PRICE_INCREASE: u64 = 2;

        let num_pre_exec_checks_needed = Self::num_pre_exec_checks_needed(blob);
        let estimated_bytes_to_store =
            ValidatedBlob::conservative_serialized_size(blob, sender) as u64;

        // In the worst case that we handle, the gas price will double - so we need to reserve enough funds to cover the pre exec checks one more time.
        let worst_case_increase_in_pre_exec_checks_gas = <S as GasSpec>::max_tx_check_costs()
            .checked_scalar_product(num_pre_exec_checks_needed)?;
        let worst_case_increase_in_pre_exec_checks_tokens =
            worst_case_increase_in_pre_exec_checks_gas.checked_value(current_gas_price)?;

        // We'll store the blob now, so we'll pay at the current gas price
        let fixed_cost_of_storing =
            <S as GasSpec>::gas_to_charge_for_cold_access().checked_value(current_gas_price)?;
        let variable_cost_of_storing = <S as GasSpec>::gas_to_charge_per_byte_for_cold_write()
            .checked_scalar_product(estimated_bytes_to_store)?
            .checked_value(current_gas_price)?;
        let tokens_needed_for_storage =
            fixed_cost_of_storing.checked_add(variable_cost_of_storing)?;

        // When we retrieve the bloh later, we'll pay some future gas price. We reserve enough funds for price to double - if it goes by more than that, we'll have to
        // drop the blob and the sequencer will be out some gas fees.
        let fixed_cost_of_retrieval = <S as GasSpec>::gas_to_charge_for_cold_access()
            .checked_scalar_product(WORST_CASE_GAS_PRICE_INCREASE)?
            .checked_value(current_gas_price)?;
        let variable_cost_of_retrieval = <S as GasSpec>::gas_to_charge_per_byte_for_cold_load()
            .checked_scalar_product(WORST_CASE_GAS_PRICE_INCREASE * estimated_bytes_to_store)?
            .checked_value(current_gas_price)?;
        let tokens_needed_for_retrieval =
            fixed_cost_of_retrieval.checked_add(variable_cost_of_retrieval)?;

        // When we delete the blob, we'll pay the future gas price - but it'll be hot because we delete at the same time we retrieve.
        let delete_cost = <S as GasSpec>::gas_to_charge_for_hot_delete()
            .checked_scalar_product(WORST_CASE_GAS_PRICE_INCREASE)?;
        let tokens_needed_for_deletion = delete_cost.checked_value(current_gas_price)?;

        tokens_needed_for_storage
            .checked_add(tokens_needed_for_retrieval)?
            .checked_add(tokens_needed_for_deletion)?
            .checked_add(worst_case_increase_in_pre_exec_checks_tokens)
    }
}
