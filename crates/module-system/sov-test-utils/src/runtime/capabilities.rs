pub use sov_sequencer_registry::SequencerStakeMeter;

/// Provides a default implementation of each requested capability for the runtime.
/// Requires the runtime to implement MinimalRuntime.
#[macro_export]
macro_rules! impl_runtime_capability {
    ($runtime:ty, RuntimeAuthenticator) => {
        impl<S, Da> ::sov_modules_api::capabilities::RuntimeAuthenticator<S> for $runtime
        where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
        {
            type Decodable = <$runtime as ::sov_modules_api::DispatchCall>::Decodable;
            type SequencerStakeMeter = $crate::runtime::capabilities::SequencerStakeMeter<S::Gas>;
            type AuthorizationData = ::sov_modules_api::capabilities::AuthorizationData<S>;

            fn authenticate(
                &self,
                raw_tx: &::sov_modules_api::RawTx,
                pre_exec_ws: &mut ::sov_modules_api::PreExecWorkingSet<
                    S,
                    Self::SequencerStakeMeter,
                >,
            ) -> ::sov_modules_api::capabilities::AuthenticationResult<
                S,
                Self::Decodable,
                Self::AuthorizationData,
            > {
                ::sov_modules_api::capabilities::authenticate::<S, Self, Self::SequencerStakeMeter>(
                    &raw_tx.data,
                    pre_exec_ws,
                )
            }

            fn authenticate_unregistered(
                &self,
                raw_tx: &::sov_modules_api::RawTx,
                pre_exec_ws: &mut ::sov_modules_api::PreExecWorkingSet<
                    S,
                    ::sov_modules_api::UnlimitedGasMeter<S::Gas>,
                >,
            ) -> ::sov_modules_api::capabilities::AuthenticationResult<
                S,
                Self::Decodable,
                Self::AuthorizationData,
                ::sov_modules_api::capabilities::UnregisteredAuthenticationError,
            > {
                ::core::result::Result::Ok(::sov_modules_api::capabilities::authenticate::<
                    S,
                    Self,
                    ::sov_modules_api::UnlimitedGasMeter<S::Gas>,
                >(&raw_tx.data, pre_exec_ws)?)
            }
        }
    };
    ($runtime:ty, GasEnforcer) => {
        impl<S, Da> ::sov_modules_api::capabilities::GasEnforcer<S, Da> for $runtime
        where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
            $runtime: $crate::runtime::traits::StandardRuntime<S, Da>,
        {
            /// Reserves enough gas for the transaction to be processed, if possible.
            fn try_reserve_gas<Meter: ::sov_modules_api::GasMeter<S::Gas>>(
                &self,
                tx: &::sov_modules_api::transaction::AuthenticatedTransactionData<S>,
                sender: &S::Address,
                pre_exec_working_set: ::sov_modules_api::PreExecWorkingSet<S, Meter>,
            ) -> Result<
                ::sov_modules_api::WorkingSet<S>,
                ::sov_modules_api::capabilities::TryReserveGasError<S, Meter>,
            > {
                self.bank
                    .reserve_gas(tx, sender, pre_exec_working_set)
                    .map_err(Into::into)
            }

            fn allocate_consumed_gas(
                &self,
                tx_consumption: &::sov_modules_api::transaction::TransactionConsumption<S::Gas>,
                tx_scratchpad: &mut ::sov_modules_api::TxScratchpad<S>,
            ) {
                use ::sov_modules_api::ModuleInfo as _;
                use $crate::runtime::traits::MinimalRuntime;
                use $crate::runtime::IntoPayable as _;

                self.bank().allocate_consumed_gas(
                    &self.base_fee_recipient(),
                    &self.sequencer_registry().id().to_payable(),
                    tx_consumption,
                    tx_scratchpad,
                );
            }

            fn refund_remaining_gas(
                &self,
                context: &::sov_modules_api::Context<S>,
                tx_consumption: &::sov_modules_api::transaction::TransactionConsumption<S::Gas>,
                tx_scratchpad: &mut ::sov_modules_api::TxScratchpad<S>,
            ) {
                $crate::runtime::traits::MinimalRuntime::bank(self).refund_remaining_gas(
                    context.sender(),
                    tx_consumption,
                    tx_scratchpad,
                );
            }
        }
    };
    ($runtime:ty, SequencerAuthorization) => {
        impl<S, Da> ::sov_modules_api::capabilities::SequencerAuthorization<S, Da> for $runtime
        where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
            $runtime: $crate::runtime::traits::StandardRuntime<S, Da>,
        {
            type SequencerStakeMeter = $crate::runtime::capabilities::SequencerStakeMeter<S::Gas>;

            fn authorize_sequencer(
                &self,
                sequencer: &<Da as ::sov_modules_api::DaSpec>::Address,
                base_fee_per_gas: &<S::Gas as ::sov_modules_api::Gas>::Price,
                tx_scratchpad: ::sov_modules_api::TxScratchpad<S>,
            ) -> ::sov_modules_api::capabilities::AuthorizationResult<S, Self::SequencerStakeMeter>
            {
                $crate::runtime::traits::MinimalRuntime::sequencer_registry(self)
                    .authorize_sequencer(sequencer, base_fee_per_gas, tx_scratchpad)
            }

            fn penalize_sequencer(
                &self,
                sequencer: &Da::Address,
                reason: impl std::fmt::Display,
                pre_exec_working_set: ::sov_modules_api::PreExecWorkingSet<
                    S,
                    Self::SequencerStakeMeter,
                >,
            ) -> ::sov_modules_api::TxScratchpad<S> {
                $crate::runtime::traits::MinimalRuntime::sequencer_registry(self)
                    .penalize_sequencer(sequencer, reason, pre_exec_working_set)
            }
        }
    };
    ($runtime:ty, SequencerRemuneration) => {
        impl<S, Da> ::sov_modules_api::capabilities::SequencerRemuneration<S, Da> for $runtime
        where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
        {
            fn reward_sequencer(
                &self,
                sender: &Da::Address,
                reward: ::sov_modules_api::transaction::SequencerReward,
                state_checkpoint: &mut ::sov_modules_api::StateCheckpoint<S>,
            ) {
                $crate::runtime::traits::MinimalRuntime::sequencer_registry(self).reward_sequencer(
                    sender,
                    reward.into(),
                    state_checkpoint,
                );
            }

            fn slash_sequencer(
                &self,
                sender: &Da::Address,
                state_checkpoint: &mut ::sov_modules_api::StateCheckpoint<S>,
            ) {
                $crate::runtime::traits::MinimalRuntime::sequencer_registry(self)
                    .slash_sequencer(sender, state_checkpoint);
            }
        }
    };
    ($runtime:ty, RuntimeAuthorization) => {
        impl<S, Da> ::sov_modules_api::capabilities::RuntimeAuthorization<S, Da> for $runtime
        where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
        {
            type SequencerStakeMeter = $crate::runtime::capabilities::SequencerStakeMeter<S::Gas>;

            type AuthorizationData = ::sov_modules_api::capabilities::AuthorizationData<S>;

            fn check_uniqueness<Meter: ::sov_modules_api::GasMeter<S::Gas>>(
                &self,
                auth_data: &Self::AuthorizationData,
                _context: &::sov_modules_api::Context<S>,
                state: &mut ::sov_modules_api::PreExecWorkingSet<S, Meter>,
            ) -> anyhow::Result<()> {
                self.nonces.check_nonce(
                    &auth_data.credential_id,
                    auth_data.nonce,
                    state,
                )
            }

            fn resolve_context(
                &self,
                auth_tx: &Self::AuthorizationData,
                sequencer: &Da::Address,
                height: u64,
                state: &mut ::sov_modules_api::PreExecWorkingSet<S, Self::SequencerStakeMeter>,
                context: ::sov_modules_api::ExecutionContext,
            ) -> anyhow::Result<::sov_modules_api::Context<S>> {
                use $crate::runtime::traits::MinimalRuntime;
                let sequencer = self
                    .sequencer_registry()
                    .resolve_da_address(sequencer, state)?
                    .expect("Sequencer is no longer registered by the time of context resolution. This is a bug");
                let sender = self.accounts().resolve_sender_address(
                    &auth_tx.default_address,
                    &auth_tx.credential_id,
                    state,
                )?;
                Ok(::sov_modules_api::Context::new(
                    sender,
                    auth_tx.credentials.clone(),
                    sequencer,
                    height,
                    context,
                ))
            }

            fn resolve_unregistered_context(
                &self,
                auth_tx: &Self::AuthorizationData,
                height: u64,
                state: &mut ::sov_modules_api::PreExecWorkingSet<S, ::sov_modules_api::UnlimitedGasMeter<S::Gas>>,
                context: ::sov_modules_api::ExecutionContext,
            ) -> anyhow::Result<::sov_modules_api::Context<S>> {
                use $crate::runtime::traits::MinimalRuntime;
                let sender = self.accounts().resolve_sender_address(
                    &auth_tx.default_address,
                    &auth_tx.credential_id,
                    state,
                )?;
                Ok(::sov_modules_api::Context::new(
                    sender.clone(),
                    auth_tx.credentials.clone(),
                    sender,
                    height,
                    context,
                ))
            }

            fn mark_tx_attempted(
                &self,
                auth_data: &Self::AuthorizationData,
                _sequencer: &Da::Address,
                tx_scratchpad: &mut ::sov_modules_api::TxScratchpad<S>,
            ) {
                self.nonces
                    .mark_tx_attempted(&auth_data.credential_id, tx_scratchpad);
            }
        }
    };
}
