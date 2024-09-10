/// Base for generating runtimes.
/// Excludes the RuntimeAuthenticator trait to allow custom runtimes like EVM to provide their own
/// implementation.
#[macro_export]
macro_rules! generate_bare_runtime {
    (
        name: $id:ident,
        modules: [$($module_name:ident : $module_ty:path),* $(,)?],
        operating_mode: $operating_mode:path,
        minimal_genesis_config_type: $minimal_genesis_config_ty:path,
        impl_hooks: [$($hook:ident),* $(,)?],
        runtime_trait_impl_bounds: [$($runtime_trait_impl_bounds:tt)*]
    ) => {
        /// Generated test runtime implementation using the testing framework.
        #[derive(
            Default,
            Clone,
            ::sov_modules_api::Genesis,
            ::sov_modules_api::DispatchCall,
            ::sov_modules_api::Event,
            ::sov_modules_api::MessageCodec,
            ::sov_modules_api::macros::CliWallet
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
            /// The attester incentives module.
            pub attester_incentives: $crate::runtime::AttesterIncentives<S, Da>,
            /// The prover incentives module.
            pub prover_incentives: $crate::runtime::ProverIncentives<S, Da>,
            $(
                /// An external module [`$module_ty`] of the generated runtime.
                pub $module_name: $module_ty
            ),*
        }

        impl<S: ::sov_modules_api::Spec, Da: ::sov_modules_api::DaSpec> $crate::runtime::traits::MinimalGenesis<S> for $id<S, Da> {
            type Da = Da;
            fn sequencer_registry_config(config: &GenesisConfig<S, Da>) -> &<$crate::runtime::SequencerRegistry<S, Self::Da> as ::sov_modules_api::Genesis>::Config {
                &config.sequencer_registry
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
                    prover_incentives: minimal_config.prover_incentives,
                    attester_incentives: minimal_config.attester_incentives,
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
                            operating_mode: $operating_mode,
                            inner_code_commitment: Default::default(),
                            outer_code_commitment: Default::default(),
                            genesis_da_height: 0,
                        }
                    }
                }
            }

            #[allow(unused)]
            /// Creates a [`$crate::runtime::GenesisParams`] from a [`GenesisConfig`] with a custom kernel config.
            pub fn into_genesis_params_with_kernel<GenesisKernelConfig> (
                self,
                kernel_config: GenesisKernelConfig,
            ) -> $crate::runtime::GenesisParams<Self, GenesisKernelConfig>{
                $crate::runtime::GenesisParams {
                    runtime: self,
                    kernel: kernel_config,
                }
            }
        }

        $(
            $crate::impl_runtime_hook!($id<S, Da>, $hook);
        )*

        impl<S, Da> $crate::runtime::Runtime<S, Da> for $id<S, Da> where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
            $($runtime_trait_impl_bounds)*
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

        impl<S, Da> ::sov_modules_api::capabilities::HasCapabilities<S, Da> for $id<S, Da> where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
        {
            type Capabilities<'a> = $crate::runtime::StandardProvenRollupCapabilities<'a, S, Da>;

            type SequencerStakeMeter = $crate::runtime::SequencerStakeMeter<S::Gas>;

            type AuthorizationData = ::sov_modules_api::capabilities::AuthorizationData<S>;

            fn capabilities(&self) -> ::sov_modules_api::capabilities::Guard<Self::Capabilities<'_>> {
                ::sov_modules_api::capabilities::Guard::new(
                    $crate::runtime::StandardProvenRollupCapabilities {
                        bank: &self.bank,
                        sequencer_registry: &self.sequencer_registry,
                        accounts: &self.accounts,
                        nonces: &self.nonces,
                        prover_incentives: &self.prover_incentives,
                        attester_incentives: &self.attester_incentives,
                    }
                )
            }
        }

    };
}

/// Base macro used for generating runtimes.
/// Generally this should be wrapped by another macro to generate a specific concrete
/// runtime implementation, optimistic vs proving for example with a simpler interface
/// for usage in general tests.
#[macro_export]
macro_rules! generate_runtime {
    (
        name: $id:ident,
        $($rest:tt)*
    ) => {
        $crate::generate_bare_runtime! {
            name: $id,
            $($rest)*
        }

        $crate::impl_standard_runtime_authenticator!($id<S, Da>);
    };
}

/// Implements a default `RuntimeAuthenticator` that uses sov modules authentication.
#[macro_export]
macro_rules! impl_standard_runtime_authenticator {
    ($runtime:ty) => {
        /// The input for the runtime's authenticator functionality.
        #[derive(std::fmt::Debug, Clone, ::borsh::BorshDeserialize, ::borsh::BorshSerialize)]
        pub struct AuthenticatorInput(::sov_modules_api::RawTx);

        impl<S, Da> ::sov_modules_api::capabilities::RuntimeAuthenticator<S> for $runtime
        where
            S: ::sov_modules_api::Spec,
            Da: ::sov_modules_api::DaSpec,
        {
            type Decodable = <$runtime as ::sov_modules_api::DispatchCall>::Decodable;
            type SequencerStakeMeter = $crate::runtime::SequencerStakeMeter<S::Gas>;
            type AuthorizationData = ::sov_modules_api::capabilities::AuthorizationData<S>;
            type Input = AuthenticatorInput;

            fn authenticate(
                &self,
                tx: &AuthenticatorInput,
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
                    &tx.0.data,
                    pre_exec_ws,
                )
            }

            fn authenticate_unregistered(
                &self,
                tx: &AuthenticatorInput,
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
                >(&tx.0.data, pre_exec_ws)?)
            }

            fn add_standard_auth(tx: ::sov_modules_api::RawTx) -> Self::Input {
                AuthenticatorInput(tx)
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
            operating_mode: $crate::runtime::OperatingMode::Optimistic,
            minimal_genesis_config_type: $crate::runtime::genesis::optimistic::config::MinimalOptimisticGenesisConfig<S, Da>,
            impl_hooks: [SlotHooks, FinalizeHook, ApplyBatchHooks, TxHooks],
            runtime_trait_impl_bounds: []
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
            operating_mode: $crate::runtime::OperatingMode::Zk,
            minimal_genesis_config_type: $crate::runtime::genesis::zk::MinimalZkGenesisConfig<S, Da>,
            impl_hooks: [SlotHooks, FinalizeHook, ApplyBatchHooks, TxHooks],
            runtime_trait_impl_bounds: []
        }
    };
}

/// Assert that a pattern matches the expected value.
/// This should be replaced by `std` version when it is stabilized: `<https://github.com/rust-lang/rust/issues/82775>`
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
