/// Base macro used for generating runtimes.
/// Generally this should be wrapped by another macro to generate a specific concrete
/// runtime implementation, optimistic vs proving for example with a simpiler interface
/// for usage in general tests.
#[macro_export]
macro_rules! generate_runtime {
    (
        name: $id:ident,
        modules: [$($module_name:ident : $module_ty:path),* $(,)?],
        base_fee_recipient: $base_fee_recipient:ident : $base_fee_recipient_ty:path,
        minimal_genesis_config_type: $minimal_genesis_config_ty:path,
        impl_capabilities: [$($capability:ident),* $(,)?],
        impl_hooks: [$($hook:ident),* $(,)?]
    ) => {
        /// Generated test runtime implementation using the testing framework.
        #[derive(
            Default,
            Clone,
            ::sov_modules_api::Genesis,
            ::sov_modules_api::DispatchCall,
            ::sov_modules_api::Event,
            ::sov_modules_api::MessageCodec
        )]
        pub struct $id<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> {
            /// The sequencer registry module.
            pub sequencer_registry: $crate::runtime::SequencerRegistry<S, Da>,
            /// The bank module.
            pub bank: $crate::runtime::Bank<S>,
            /// The accounts module
            pub accounts: $crate::runtime::Accounts<S>,
            /// The nonces module
            pub nonces: $crate::runtime::Nonces<S>,
            /// The module that will receive the base fee.
            pub $base_fee_recipient: $base_fee_recipient_ty,
            $(
                /// An external module [`$module_ty`] of the generated runtime.
                pub $module_name: $module_ty
            ),*
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> $crate::runtime::traits::MinimalRuntime<S, Da> for $id<S, Da> {
            fn bank(&self) -> &$crate::runtime::Bank<S> {
                &self.bank
            }

            fn sequencer_registry(&self) -> &$crate::runtime::SequencerRegistry<S, Da> {
                &self.sequencer_registry
            }

            fn base_fee_recipient(&self) -> impl $crate::runtime::Payable<S> {
                $crate::runtime::IntoPayable::to_payable(&self.$base_fee_recipient.id)
            }

            fn accounts(&self) -> &$crate::runtime::Accounts<S> {
                &self.accounts
            }

            fn nonces(&self) -> &$crate::runtime::Nonces<S> {
                &self.nonces
            }
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> $crate::runtime::traits::MinimalGenesis<S> for $id<S, Da> {
            type Da = Da;
            fn sequencer_registry_config(config: &GenesisConfig<S, Da>) -> &<$crate::runtime::SequencerRegistry<S, Self::Da> as ::sov_modules_api::Genesis>::Config {
                &config.sequencer_registry
            }

            fn bank_config(config: &GenesisConfig<S, Da>) -> &<$crate::runtime::Bank<S> as ::sov_modules_api::Genesis>::Config {
                &config.bank
            }

            fn accounts_config(config: &GenesisConfig<S, Da>) -> &<$crate::runtime::Accounts<S> as ::sov_modules_api::Genesis>::Config {
                &config.accounts
            }
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> GenesisConfig<S, Da> {
            #[allow(unused)]
            /// Creates a new [`GenesisConfig`] from a minimal genesis config [`::sov_modules_api::Genesis::Config`].
            pub fn from_minimal_config(minimal_config: $minimal_genesis_config_ty,
                $($module_name: <$module_ty as ::sov_modules_api::Genesis>::Config),*
            ) -> Self {
                Self {
                    sequencer_registry: minimal_config.sequencer_registry,
                    bank: minimal_config.bank,
                    accounts: minimal_config.accounts,
                    nonces: minimal_config.nonces,
                    $base_fee_recipient: minimal_config.$base_fee_recipient,
                    $(
                        $module_name,
                    )*
                }
            }
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> GenesisConfig<S, Da>
        where <S::InnerZkvm as ::sov_modules_api::Zkvm>::CodeCommitment: Default,
         <S::OuterZkvm as ::sov_modules_api::Zkvm>::CodeCommitment: Default,{
            #[allow(unused)]
            /// Creates a [`$crate::runtime::GenesisParams`] from a [`GenesisConfig`].
            pub fn into_genesis_params(self) -> $crate::runtime::GenesisParams<Self, $crate::runtime::BasicKernelGenesisConfig<S, Da>> {
                $crate::runtime::GenesisParams {
                    runtime: self,
                    kernel: $crate::runtime::BasicKernelGenesisConfig {
                        chain_state: $crate::runtime::ChainStateConfig {
                            current_time: Default::default(),
                            inner_code_commitment: Default::default(),
                            outer_code_commitment: Default::default(),
                            genesis_da_height: 0,
                        }
                    }
                }
            }
        }

        $(
            $crate::impl_runtime_capability!($id<S, Da>, $capability);
        )*

        $(
            $crate::impl_runtime_hook!($id<S, Da>, $hook);
        )*

        impl<S, Da> ::sov_modules_api::capabilities::HasCapabilities<S, Da> for $id<S, Da> where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
        {
            type Capabilities<'a> = Self
                where
                Self: 'a,;
            type SequencerStakeMeter = $crate::runtime::capabilities::SequencerStakeMeter<S::Gas>;

            type AuthorizationData = ::sov_modules_api::capabilities::AuthorizationData<S>;

            fn capabilities(&self) -> Self::Capabilities<'_> {
                Self::default()
            }
        }

        impl<S, Da> $crate::runtime::Runtime<S, Da> for $id<S, Da> where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
        {
            type GenesisConfig = <Self as ::sov_modules_api::Genesis>::Config;

            type GenesisPaths = ();

            fn endpoints(
                _storage: $crate::runtime::Receiver<S::Storage>,
            ) -> $crate::runtime::RuntimeEndpoints {
                unimplemented!()
            }

            fn genesis_config(_genesis_paths: &Self::GenesisPaths) -> anyhow::Result<Self::GenesisConfig> {
                unimplemented!()
            }
        }
    };
}

/// Generates a optimistic runtime containing the [`Bank`](sov_bank::Bank), [`AttesterIncentives`](sov_attester_incentives::AttesterIncentives),
/// and [`SequencerRegistry`](sov_sequencer_registry::SequencerRegistry) modules in addition to any provided as arguments.
#[macro_export]
macro_rules! generate_optimistic_runtime {
    ($id:ident <= $($module_name:ident : $module_ty:path),*) => {
        $crate::generate_runtime! {
            name: $id,
            modules: [$($module_name : $module_ty),*],
            base_fee_recipient: attester_incentives: $crate::runtime::AttesterIncentives<S, Da>,
            minimal_genesis_config_type: $crate::runtime::genesis::optimistic::config::MinimalOptimisticGenesisConfig<S, Da>,
            impl_capabilities: [RuntimeAuthenticator, GasEnforcer, SequencerAuthorization, SequencerRemuneration, RuntimeAuthorization],
            impl_hooks: [SlotHooks, FinalizeHook, ApplyBatchHooks, TxHooks]
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> ::sov_modules_api::capabilities::ProofProcessor<S, Da> for $id<S, Da> {
            fn process_aggregated_proof(
                &self,
                _proof: ::sov_modules_api::SerializedAggregatedProof,
                _prover_address: &S::Address,
                _state: &mut ::sov_modules_api::WorkingSet<S>,
            ) -> ::sov_modules_api::SovProofOutcome<S, Da> {
                ::sov_modules_api::ProofOutcome::Ignored
            }


            fn process_attestation(
                &self,
                proof: ::sov_modules_api::SerializedAttestation,
                prover_address: &S::Address,
                state: &mut ::sov_modules_api::WorkingSet<S>,
            ) -> ::sov_modules_api::SovProofOutcome<S, Da> {
                match self.attester_incentives.process_attestation(prover_address, proof, state) {
                    Ok(attestation) => ::sov_modules_api::ProofOutcome::Valid(
                        ::sov_modules_api::ProofReceiptContents::Attestation(attestation)
                    ),
                    Err(e) => {
                        ::sov_modules_api::ProofOutcome::Invalid(e.into())
                    }
                }
            }

            fn process_challenge(
                &self,
                proof: ::sov_modules_api::SerializedChallenge,
                transition_num: u64,
                prover_address: &S::Address,
                state: &mut ::sov_modules_api::WorkingSet<S>,
            ) -> ::sov_modules_api::SovProofOutcome<S, Da> {
                match self.attester_incentives.process_challenge(prover_address,&proof, transition_num, state) {
                    Ok(Some(challenge)) => ::sov_modules_api::ProofOutcome::Valid(
                        ::sov_modules_api::ProofReceiptContents::BlockProof(challenge)
                    ),
                    Ok(None) => ::sov_modules_api::ProofOutcome::Ignored,
                    Err(e) => {
                        ::sov_modules_api::ProofOutcome::Invalid(e.into())
                    }
                }
            }

        }
    };
}

/// Generates a zk runtime containing the [`Bank`](sov_bank::Bank), [`ProverIncentives`](sov_prover_incentives::ProverIncentives),
/// and [`SequencerRegistry`](sov_sequencer_registry::SequencerRegistry) modules in addition to any provided as arguments.
#[macro_export]
macro_rules! generate_zk_runtime {
    ($id:ident <= $($module_name:ident : $module_ty:path),*) => {
        $crate::generate_runtime! {
            name: $id,
            modules: [$($module_name : $module_ty),*],
            base_fee_recipient: prover_incentives: $crate::runtime::ProverIncentives<S, Da>,
            minimal_genesis_config_type: $crate::runtime::genesis::zk::MinimalZkGenesisConfig<S, Da>,
            impl_capabilities: [RuntimeAuthenticator, GasEnforcer, SequencerAuthorization, SequencerRemuneration, RuntimeAuthorization],
            impl_hooks: [SlotHooks, FinalizeHook, ApplyBatchHooks, TxHooks]
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> ::sov_modules_api::capabilities::ProofProcessor<S, Da> for $id<S, Da> {
            fn process_aggregated_proof(
                &self,
                proof: ::sov_modules_api::SerializedAggregatedProof,
                prover_address: &S::Address,
                state: &mut ::sov_modules_api::WorkingSet<S>,
            ) -> ::sov_modules_api::SovProofOutcome<S, Da> {
                match self.prover_incentives.process_proof(&proof, prover_address, state) {
                    Ok(data) => ::sov_modules_api::ProofOutcome::Valid(
                        ::sov_modules_api::ProofReceiptContents::AggregateProof(data, proof)
                    ),
                    Err(e) => {
                        ::sov_modules_api::ProofOutcome::Invalid(e.into())
                    }
                }
            }

            fn process_attestation(
                &self,
                _proof: ::sov_modules_api::SerializedAttestation,
                _prover_address: &S::Address,
                _state: &mut ::sov_modules_api::WorkingSet<S>,
            ) -> ::sov_modules_api::SovProofOutcome<S, Da> {
                ::sov_modules_api::ProofOutcome::Ignored
            }

            fn process_challenge(
                &self,
                _proof: ::sov_modules_api::SerializedChallenge,
                _transition_num: u64,
                _prover_address: &S::Address,
                _state: &mut ::sov_modules_api::WorkingSet<S>,
            ) -> ::sov_modules_api::SovProofOutcome<S, Da> {
                ::sov_modules_api::ProofOutcome::Ignored
            }
        }
    };
}

/// Assert that a pattern matches the expected value.
/// This should be replaced by `std` version when it is stablized: `<https://github.com/rust-lang/rust/issues/82775>`
#[macro_export]
macro_rules! assert_matches {
    ($value:expr, $pattern:pat) => {
        assert_matches!($value, $pattern, "")
    };
    ($value:expr, $pattern:pat if $guard:expr) => {
        assert_matches!($value, $pattern if $guard, "")
    };
    ($value:expr, $pattern:pat, $message:expr) => {{
        match $value {
            $pattern => (),
            ref _v => panic!(
                "{}Assertion failed:\nExpected: {}\nReceived: {:?}",
                if $message.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", $message)
                },
                stringify!($pattern),
                _v
            ),
        }
    }};
    ($value:expr, $pattern:pat if $guard:expr, $message:expr) => {{
        match $value {
            v @ $pattern => {
                if !($guard) {
                    panic!(
                        "{}Assertion failed:\nExpected: {} if {}\nReceived: {:?}",
                        if $message.is_empty() {
                            String::new()
                        } else {
                            format!("{}\n", $message)
                        },
                        stringify!($pattern),
                        stringify!($guard),
                        v
                    )
                }
            }
            ref _v => panic!(
                "{}Assertion failed:\nExpected: {} if {}\nReceived: {:?}",
                if $message.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", $message)
                },
                stringify!($pattern),
                stringify!($guard),
                _v
            ),
        }
    }};
}
