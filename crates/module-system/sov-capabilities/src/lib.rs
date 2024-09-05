use sov_bank::IntoPayable;
use sov_modules_api::capabilities::{
    AuthorizationData, AuthorizationResult, GasEnforcer, ProofProcessor, RuntimeAuthorization,
    SequencerAuthorization, SequencerRemuneration, TryReserveGasError,
};
use sov_modules_api::transaction::{
    AuthenticatedTransactionData, SequencerReward, TransactionConsumption,
};
use sov_modules_api::{
    AggregatedProofPublicData, Context, DaSpec, ExecutionContext, Gas, GasMeter, InvalidProofError,
    ModuleInfo, PreExecWorkingSet, SovAttestation, SovStateTransitionPublicData, Spec,
    StateCheckpoint, TxScratchpad, UnlimitedGasMeter, WorkingSet,
};
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;
use sov_sequencer_registry::{SequencerRegistry, SequencerStakeMeter};

/// Implements the basic capabilities required for a zk-rollup runtime.
pub struct StandardProvenRollupCapabilities<'a, S: Spec, Da: DaSpec> {
    pub bank: &'a sov_bank::Bank<S>,
    pub sequencer_registry: &'a SequencerRegistry<S, Da>,
    pub accounts: &'a sov_accounts::Accounts<S>,
    pub nonces: &'a sov_nonces::Nonces<S>,
    pub prover_incentives: &'a sov_prover_incentives::ProverIncentives<S, Da>,
    pub attester_incentives: &'a sov_attester_incentives::AttesterIncentives<S, Da>,
}

impl<'a, S: Spec, Da: DaSpec> GasEnforcer<S, Da> for StandardProvenRollupCapabilities<'a, S, Da> {
    /// Reserves enough gas for the transaction to be processed, if possible.
    fn try_reserve_gas<Meter: GasMeter<S::Gas>>(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        sender: &S::Address,
        pre_exec_working_set: PreExecWorkingSet<S, Meter>,
    ) -> Result<WorkingSet<S>, TryReserveGasError<S, Meter>> {
        self.bank
            .reserve_gas(tx, sender, pre_exec_working_set)
            .map_err(Into::into)
    }

    fn allocate_consumed_gas(
        &self,
        tx_consumption: &TransactionConsumption<S::Gas>,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) {
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

        // TODO(@theochap): In the next PR this method will become failible
        self.bank
            .allocate_consumed_gas(&rewarded_module, tx_consumption, tx_scratchpad);
    }

    fn refund_remaining_gas(
        &self,
        sender: &S::Address,
        tx_consumption: &TransactionConsumption<S::Gas>,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) {
        self.bank
            .refund_remaining_gas(sender, tx_consumption, tx_scratchpad);
    }
}

impl<'a, S: Spec, Da: DaSpec> SequencerAuthorization<S, Da>
    for StandardProvenRollupCapabilities<'a, S, Da>
{
    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    fn authorize_sequencer(
        &self,
        sequencer: &<Da as DaSpec>::Address,
        base_fee_per_gas: &<S::Gas as Gas>::Price,
        tx_scratchpad: TxScratchpad<S::Storage>,
    ) -> AuthorizationResult<S, Self::SequencerStakeMeter> {
        self.sequencer_registry
            .authorize_sequencer(sequencer, base_fee_per_gas, tx_scratchpad)
    }

    fn penalize_sequencer(
        &self,
        sequencer: &Da::Address,
        reason: impl std::fmt::Display,
        pre_exec_working_set: PreExecWorkingSet<S, Self::SequencerStakeMeter>,
    ) -> TxScratchpad<S::Storage> {
        self.sequencer_registry
            .penalize_sequencer(sequencer, reason, pre_exec_working_set)
    }
}

impl<'a, S: Spec, Da: DaSpec> RuntimeAuthorization<S, Da>
    for StandardProvenRollupCapabilities<'a, S, Da>
{
    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;
    type AuthorizationData = AuthorizationData<S>;

    /// Prevents duplicate transactions from running.
    // TODO(@preston-evans98): Use type system to prevent writing to the `StateCheckpoint` during this check
    fn check_uniqueness<Meter: GasMeter<S::Gas>>(
        &self,
        auth_data: &Self::AuthorizationData,
        _context: &Context<S>,
        pre_exec_working_set: &mut PreExecWorkingSet<S, Meter>,
    ) -> anyhow::Result<()> {
        self.nonces.check_nonce(
            &auth_data.credential_id,
            auth_data.nonce,
            pre_exec_working_set,
        )
    }

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        auth_data: &Self::AuthorizationData,
        _sequencer: &Da::Address,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) {
        self.nonces
            .mark_tx_attempted(&auth_data.credential_id, tx_scratchpad);
    }

    /// Resolves the context for a transaction.
    fn resolve_context(
        &self,
        auth_data: &Self::AuthorizationData,
        sequencer: &Da::Address,
        height: u64,
        state: &mut PreExecWorkingSet<S, Self::SequencerStakeMeter>,
        execution_context: ExecutionContext,
    ) -> anyhow::Result<Context<S>> {
        // TODO(@preston-evans98): This is a temporary hack to get the sequencer address
        // This should be resolved by the sequencer registry during blob selection
        let sequencer = self.
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
            sequencer,
            height,
            execution_context,
        ))
    }

    fn resolve_unregistered_context(
        &self,
        auth_data: &Self::AuthorizationData,
        height: u64,
        state: &mut PreExecWorkingSet<S, UnlimitedGasMeter<S::Gas>>,
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
            height,
            execution_context,
        ))
    }
}

impl<'a, S: Spec, Da: DaSpec> ProofProcessor<S, Da>
    for StandardProvenRollupCapabilities<'a, S, Da>
{
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
    ) -> Result<SovAttestation<S, Da>, InvalidProofError> {
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
    ) -> Result<SovStateTransitionPublicData<S, Da>, InvalidProofError> {
        let result = self.attester_incentives.process_challenge(
            prover_address,
            &proof,
            rollup_height,
            state,
        )?;

        Ok(result)
    }
}

impl<'a, S: Spec, Da: DaSpec> SequencerRemuneration<S, Da>
    for StandardProvenRollupCapabilities<'a, S, Da>
{
    fn reward_sequencer(
        &self,
        sender: &S::Address,
        reward: SequencerReward,
        state: &mut TxScratchpad<S::Storage>,
    ) {
        self.sequencer_registry
            .reward_sequencer(sender, reward.into(), state);
    }

    fn slash_sequencer(
        &self,
        sender: &Da::Address,
        state_checkpoint: &mut StateCheckpoint<S::Storage>,
    ) {
        self.sequencer_registry
            .slash_sequencer(sender, state_checkpoint);
    }
}
