use sov_bank::utils::TokenHolderRef;
use sov_bank::IntoPayable;
use sov_modules_api::capabilities::{
    AllowedSequencer, AuthorizationData, AuthorizeSequencerError, GasEnforcer, ProofProcessor,
    SequencerAuthorization, SequencerRemuneration, TransactionAuthorizer, TryReserveGasError,
};
use sov_modules_api::transaction::{
    AuthenticatedTransactionData, ProverRewards, RemainingFunds, SequencerReward,
};
use sov_modules_api::{
    AggregatedProofPublicData, Context, DaSpec, ExecutionContext, Gas, InvalidProofError,
    ModuleInfo, SovAttestation, SovStateTransitionPublicData, Spec, TxScratchpad, WorkingSet,
};
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;
use sov_sequencer_registry::SequencerRegistry;

/// Implements the basic capabilities required for a zk-rollup runtime.
pub struct StandardProvenRollupCapabilities<'a, S: Spec> {
    pub bank: &'a sov_bank::Bank<S>,
    pub sequencer_registry: &'a SequencerRegistry<S>,
    pub accounts: &'a sov_accounts::Accounts<S>,
    pub nonces: &'a sov_nonces::Nonces<S>,
    pub prover_incentives: &'a sov_prover_incentives::ProverIncentives<S>,
    pub attester_incentives: &'a sov_attester_incentives::AttesterIncentives<S>,
}

impl<'a, S: Spec> StandardProvenRollupCapabilities<'a, S> {
    fn get_prover_token_holder(
        &self,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> TokenHolderRef<'a, S> {
        let reward_prover_incentives = self.prover_incentives.should_reward_fees(tx_scratchpad);
        let reward_attester_incentives = self.attester_incentives.should_reward_fees(tx_scratchpad);

        assert!(
            reward_prover_incentives ^ reward_attester_incentives,
            "Exactly one of prover or attester incentives should be rewarded"
        );

        let rewarded_module = if reward_prover_incentives {
            self.prover_incentives.id().to_payable()
        } else {
            self.attester_incentives.id().to_payable()
        };

        rewarded_module
    }
}

impl<'a, S: Spec> GasEnforcer<S> for StandardProvenRollupCapabilities<'a, S> {
    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        context: &Context<S>,
        scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> Result<(), TryReserveGasError> {
        self.bank
            .reserve_gas(tx, gas_price, context.sender(), scratchpad)
            .map_err(Into::into)
    }

    fn try_reserve_gas_for_proof(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        sender: &S::Address,
        scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> Result<(), TryReserveGasError> {
        self.bank
            .reserve_gas(tx, gas_price, sender, scratchpad)
            .map_err(Into::into)
    }

    fn reward_prover(
        &self,
        prover_rewards: &ProverRewards,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) {
        let rewarded_module = self.get_prover_token_holder(tx_scratchpad);

        self.bank
            .reward_prover(&rewarded_module, prover_rewards, tx_scratchpad);
    }

    fn refund_remaining_gas(
        &self,
        sender: &S::Address,
        remaining_funds: &RemainingFunds,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) {
        self.bank
            .refund_remaining_gas(sender, remaining_funds, tx_scratchpad);
    }

    fn transfer_authentication_cost_from_sequencer_to_prover(
        &self,
        amount: u64,
        sequencer: &<S::Da as DaSpec>::Address,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) {
        let rewarded_prover_module = self.get_prover_token_holder(tx_scratchpad);
        self.sequencer_registry
            .remove_part_of_the_stake(sequencer, rewarded_prover_module, amount, tx_scratchpad)
            .unwrap_or_else(|e| panic!("Unable to remove the sequencer's stake: {}", e));
    }

    fn transfer_authentication_cost_from_user_to_sequencer(
        &self,
        amount: u64,
        user: &S::Address,
        sequencer: &<S::Da as DaSpec>::Address,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) {
        self.sequencer_registry
            .add_to_stake(user, sequencer, amount, tx_scratchpad)
            .unwrap_or_else(|e| panic!("Unable to increase the sequencer's stake {}", e));
    }
}

impl<'a, S: Spec> SequencerAuthorization<S> for StandardProvenRollupCapabilities<'a, S> {
    fn authorize_sequencer(
        &self,
        sequencer: &<S::Da as DaSpec>::Address,
        base_fee_per_gas: &<S::Gas as Gas>::Price,
        state: &mut TxScratchpad<S::Storage>,
    ) -> Result<AllowedSequencer<S>, AuthorizeSequencerError> {
        self.sequencer_registry
            .authorize_sequencer(sequencer, base_fee_per_gas, state)
    }

    fn penalize_sequencer(
        &self,
        sequencer: &<S::Da as DaSpec>::Address,
        reason: impl std::fmt::Display,
        remaining_stake: u64,
        state: &mut TxScratchpad<S::Storage>,
    ) {
        self.sequencer_registry
            .penalize_sequencer(sequencer, reason, remaining_stake, state);
    }
}

impl<'a, S: Spec> TransactionAuthorizer<S> for StandardProvenRollupCapabilities<'a, S> {
    type AuthorizationData = AuthorizationData<S>;

    /// Prevents duplicate transactions from running.
    // TODO(@preston-evans98): Use type system to prevent writing to the `StateCheckpoint` during this check
    fn check_uniqueness(
        &self,
        auth_data: &Self::AuthorizationData,
        _context: &Context<S>,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> anyhow::Result<()> {
        self.nonces
            .check_nonce(&auth_data.credential_id, auth_data.nonce, tx_scratchpad)
    }

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        auth_data: &Self::AuthorizationData,
        _sequencer: &<S::Da as DaSpec>::Address,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) {
        self.nonces
            .mark_tx_attempted(&auth_data.credential_id, tx_scratchpad);
    }

    /// Resolves the context for a transaction.
    fn resolve_context(
        &self,
        auth_data: &Self::AuthorizationData,
        sequencer: &<S::Da as DaSpec>::Address,
        height: u64,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
        execution_context: ExecutionContext,
    ) -> anyhow::Result<Context<S>> {
        // TODO(@preston-evans98): This is a temporary hack to get the sequencer address
        // This should be resolved by the sequencer registry during blob selection
        let sequencer = self.
        sequencer_registry.resolve_da_address(sequencer, tx_scratchpad)?
            .ok_or(anyhow::anyhow!("Sequencer was no longer registered by the time of context resolution. This is a bug")).unwrap();
        let sender = self.accounts.resolve_sender_address(
            &auth_data.default_address,
            &auth_data.credential_id,
            tx_scratchpad,
        )?;
        Ok(Context::new(
            sender,
            auth_data.credentials.clone(),
            sequencer,
            height,
            execution_context,
        ))
    }

    fn resolve_unregistered_context(
        &self,
        auth_data: &Self::AuthorizationData,
        height: u64,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
        execution_context: ExecutionContext,
    ) -> anyhow::Result<Context<S>> {
        let sender = self.accounts.resolve_sender_address(
            &auth_data.default_address,
            &auth_data.credential_id,
            tx_scratchpad,
        )?;
        // The tx sender & sequencer are the same entity
        Ok(Context::new(
            sender.clone(),
            auth_data.credentials.clone(),
            sender,
            height,
            execution_context,
        ))
    }
}

impl<'a, S: Spec> ProofProcessor<S> for StandardProvenRollupCapabilities<'a, S> {
    fn process_aggregated_proof(
        &self,
        proof: SerializedAggregatedProof,
        prover_address: &S::Address,
        state: &mut WorkingSet<S>,
    ) -> Result<(AggregatedProofPublicData, SerializedAggregatedProof), InvalidProofError> {
        let result = self
            .prover_incentives
            .process_proof(&proof, prover_address, state)?;

        Ok((result, proof))
    }

    fn process_attestation(
        &self,
        proof: sov_rollup_interface::optimistic::SerializedAttestation,
        prover_address: &<S as Spec>::Address,
        state: &mut WorkingSet<S>,
    ) -> Result<SovAttestation<S>, InvalidProofError> {
        let result = self
            .attester_incentives
            .process_attestation(prover_address, proof, state)?;

        Ok(result)
    }

    fn process_challenge(
        &self,
        proof: sov_rollup_interface::optimistic::SerializedChallenge,
        rollup_height: u64,
        prover_address: &<S as Spec>::Address,
        state: &mut WorkingSet<S>,
    ) -> Result<SovStateTransitionPublicData<S>, InvalidProofError> {
        let result = self.attester_incentives.process_challenge(
            prover_address,
            &proof,
            rollup_height,
            state,
        )?;

        Ok(result)
    }
}

impl<'a, S: Spec> SequencerRemuneration<S> for StandardProvenRollupCapabilities<'a, S> {
    fn reward_sequencer(
        &self,
        sequencer: &<S::Da as DaSpec>::Address,
        reward: SequencerReward,
        state: &mut TxScratchpad<S::Storage>,
    ) {
        self.sequencer_registry
            .add_to_stake(self.bank.id().to_payable(), sequencer, reward.into(), state)
            .unwrap_or_else(|e| panic!("Unable to increase the sequencer's stake {}", e));
    }
}
