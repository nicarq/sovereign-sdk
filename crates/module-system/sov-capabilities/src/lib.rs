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
    AggregatedProofPublicData, Context, DaSpec, ExecutionContext, Gas, InfallibleStateAccessor,
    InvalidProofError, ModuleInfo, SovAttestation, SovStateTransitionPublicData, Spec, TxState,
};
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;
use sov_sequencer_registry::SequencerRegistry;

/// Implements the basic capabilities required for a zk-rollup runtime.
pub struct StandardProvenRollupCapabilities<'a, S: Spec, GasPayer = sov_bank::Bank<S>> {
    pub bank: &'a sov_bank::Bank<S>,
    pub gas_payer: &'a GasPayer,
    pub sequencer_registry: &'a SequencerRegistry<S>,
    pub accounts: &'a sov_accounts::Accounts<S>,
    pub nonces: &'a sov_nonces::Nonces<S>,
    pub prover_incentives: &'a sov_prover_incentives::ProverIncentives<S>,
    pub attester_incentives: &'a sov_attester_incentives::AttesterIncentives<S>,
}

impl<'a, S: Spec, T> StandardProvenRollupCapabilities<'a, S, T> {
    fn get_prover_token_holder(
        &self,
        state: &mut impl InfallibleStateAccessor,
    ) -> TokenHolderRef<'a, S> {
        let reward_prover_incentives = self.prover_incentives.should_reward_fees(state);
        let reward_attester_incentives = self.attester_incentives.should_reward_fees(state);

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

trait HasGasPayer<S: Spec> {
    fn try_reserve_gas_from_payer(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        context: &mut Context<S>,
        state: &mut impl InfallibleStateAccessor,
    ) -> Result<(), TryReserveGasError>;
}

impl<'a, S: Spec> HasGasPayer<S> for StandardProvenRollupCapabilities<'a, S> {
    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas_from_payer(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        context: &mut Context<S>,
        scratchpad: &mut impl InfallibleStateAccessor,
    ) -> Result<(), TryReserveGasError> {
        self.gas_payer
            .reserve_gas(tx, gas_price, context.sender(), scratchpad)
            .map_err(Into::into)
    }
}

impl<'a, S: Spec> HasGasPayer<S>
    for StandardProvenRollupCapabilities<'a, S, sov_paymaster::Paymaster<S>>
{
    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas_from_payer(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        context: &mut Context<S>,
        scratchpad: &mut impl InfallibleStateAccessor,
    ) -> Result<(), TryReserveGasError> {
        self.gas_payer
            .try_reserve_gas(tx, gas_price, context, scratchpad)
            .map_err(Into::into)
    }
}

impl<'a, S: Spec, T> GasEnforcer<S> for StandardProvenRollupCapabilities<'a, S, T>
where
    Self: HasGasPayer<S>,
{
    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        context: &mut Context<S>,
        state: &mut impl InfallibleStateAccessor,
    ) -> Result<(), TryReserveGasError> {
        self.try_reserve_gas_from_payer(tx, gas_price, context, state)
            .map_err(Into::into)
    }

    fn try_reserve_gas_for_proof(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        sender: &S::Address,
        state: &mut impl InfallibleStateAccessor,
    ) -> Result<(), TryReserveGasError> {
        self.bank
            .reserve_gas(tx, gas_price, sender, state)
            .map_err(Into::into)
    }

    fn reward_prover(
        &self,
        prover_rewards: &ProverRewards,
        state: &mut impl InfallibleStateAccessor,
    ) {
        let rewarded_module = self.get_prover_token_holder(state);

        self.bank
            .reward_prover(&rewarded_module, prover_rewards, state);
    }

    fn refund_remaining_gas(
        &self,
        recipient: &S::Address,
        remaining_funds: &RemainingFunds,
        state: &mut impl InfallibleStateAccessor,
    ) {
        self.bank
            .refund_remaining_gas(recipient, remaining_funds, state);
    }

    fn transfer_funds_from_sequencer_to_prover(
        &self,
        amount: u64,
        sequencer: &<S::Da as DaSpec>::Address,
        state: &mut impl InfallibleStateAccessor,
    ) -> anyhow::Result<()> {
        let rewarded_prover_module = self.get_prover_token_holder(state);
        self.sequencer_registry.remove_part_of_the_stake(
            sequencer,
            rewarded_prover_module,
            amount,
            state,
        )
    }

    fn transfer_authentication_cost_from_user_to_sequencer(
        &self,
        amount: u64,
        user: &S::Address,
        sequencer: &<S::Da as DaSpec>::Address,
        state: &mut impl InfallibleStateAccessor,
    ) {
        self.sequencer_registry
            .add_to_stake(user, sequencer, amount, state)
            .unwrap_or_else(|e| panic!("Unable to increase the sequencer's stake {}", e));
    }
}

impl<'a, S: Spec, T> SequencerAuthorization<S> for StandardProvenRollupCapabilities<'a, S, T> {
    fn authorize_sequencer(
        &self,
        sequencer: &<S::Da as DaSpec>::Address,
        min_bond: u64,
        state: &mut impl InfallibleStateAccessor,
    ) -> Result<AllowedSequencer<S>, AuthorizeSequencerError> {
        self.sequencer_registry
            .authorize_sequencer(sequencer, min_bond, state)
    }
}

impl<'a, S: Spec, T> TransactionAuthorizer<S> for StandardProvenRollupCapabilities<'a, S, T> {
    type AuthorizationData = AuthorizationData<S>;

    /// Prevents duplicate transactions from running.
    // TODO(@preston-evans98): Use type system to prevent writing to the `StateCheckpoint` during this check
    fn check_uniqueness(
        &self,
        auth_data: &Self::AuthorizationData,
        _context: &Context<S>,
        state: &mut impl InfallibleStateAccessor,
    ) -> anyhow::Result<()> {
        self.nonces
            .check_nonce(&auth_data.credential_id, auth_data.nonce, state)
    }

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        auth_data: &Self::AuthorizationData,
        _sequencer: &<S::Da as DaSpec>::Address,
        state: &mut impl InfallibleStateAccessor,
    ) {
        self.nonces
            .mark_tx_attempted(&auth_data.credential_id, state);
    }

    /// Resolves the context for a transaction.
    fn resolve_context(
        &self,
        auth_data: &Self::AuthorizationData,
        sequencer: &<S::Da as DaSpec>::Address,
        height: u64,
        state: &mut impl InfallibleStateAccessor,
        execution_context: ExecutionContext,
    ) -> anyhow::Result<Context<S>> {
        // TODO(@preston-evans98): This is a temporary hack to get the sequencer address
        // This should be resolved by the sequencer registry during blob selection
        let sequencer_rollup_address = self.
        sequencer_registry.resolve_da_address(sequencer, state)?
            .ok_or(anyhow::anyhow!("Sequencer was no longer registered by the time of context resolution. This is a bug")).unwrap();
        let sender = self.accounts.resolve_sender_address(
            &auth_data.default_address,
            &auth_data.credential_id,
            state,
        )?;
        Ok(Context::new(
            sender,
            auth_data.credentials.clone(),
            sequencer_rollup_address,
            sequencer.clone(),
            height,
            execution_context,
        ))
    }

    fn resolve_unregistered_context(
        &self,
        auth_data: &Self::AuthorizationData,
        sequencer: &<<S as Spec>::Da as DaSpec>::Address,
        height: u64,
        state: &mut impl InfallibleStateAccessor,
        execution_context: ExecutionContext,
    ) -> anyhow::Result<Context<S>> {
        let sender = self.accounts.resolve_sender_address(
            &auth_data.default_address,
            &auth_data.credential_id,
            state,
        )?;
        // The tx sender & sequencer are the same entity
        Ok(Context::new(
            sender.clone(),
            auth_data.credentials.clone(),
            sender,
            sequencer.clone(),
            height,
            execution_context,
        ))
    }
}

impl<'a, S: Spec, T> ProofProcessor<S> for StandardProvenRollupCapabilities<'a, S, T> {
    fn process_aggregated_proof(
        &self,
        proof: SerializedAggregatedProof,
        prover_address: &S::Address,
        state: &mut impl TxState<S>,
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
        state: &mut impl TxState<S>,
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
        state: &mut impl TxState<S>,
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

impl<'a, S: Spec, T> SequencerRemuneration<S> for StandardProvenRollupCapabilities<'a, S, T> {
    fn reward_sequencer(
        &self,
        sequencer: &<S::Da as DaSpec>::Address,
        reward: SequencerReward,
        state: &mut impl InfallibleStateAccessor,
    ) {
        self.sequencer_registry
            .add_to_stake(self.bank.id().to_payable(), sequencer, reward.into(), state)
            .unwrap_or_else(|e| panic!("Unable to increase the sequencer's stake {}", e));
    }
}
