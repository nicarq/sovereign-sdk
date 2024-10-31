/// Base for generating runtimes.
/// Excludes the TransactionAuthenticator trait to allow custom runtimes like EVM to provide their own
/// implementation.
#[macro_export]
macro_rules! generate_bare_runtime {
    (
        name: $id:ident,
        modules: [$($module_name:ident : $module_ty:path),* $(,)?],
        operating_mode: $operating_mode:path,
        minimal_genesis_config_type: $minimal_genesis_config_ty:path,
        impl_hooks: [$($hook:ident),* $(,)?],
        $(gas_enforcer_override: $gas_enforcer_override_fn:ident,)?
        runtime_trait_impl_bounds: [$($runtime_trait_impl_bounds:tt)*],
        kernel_type: $kernel_type:ty
        // optional final comma
        $(,)?
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
        pub struct $id<S: ::sov_modules_api::Spec> {
            /// The sequencer registry module.
            pub sequencer_registry: $crate::runtime::SequencerRegistry<S>,
            /// The bank module.
            pub bank: $crate::runtime::Bank<S>,
            /// The accounts module
            pub accounts: $crate::runtime::Accounts<S>,
            /// The nonces module
            pub nonces: $crate::runtime::Nonces<S>,
            /// The attester incentives module.
            pub attester_incentives: $crate::runtime::AttesterIncentives<S>,
            /// The chain state module.
            pub chain_state: $crate::runtime::ChainState<S>,
            /// The blob storage module.
            pub blob_storage: $crate::runtime::BlobStorage<S>,
            /// The prover incentives module.
            pub prover_incentives: $crate::runtime::ProverIncentives<S>,
            $(
                /// An external module [`$module_ty`] of the generated runtime.
                pub $module_name: $module_ty
            ),*
        }

        impl<S: ::sov_modules_api::Spec> $crate::runtime::traits::MinimalGenesis<S> for $id<S> {
            fn sequencer_registry_config(config: &GenesisConfig<S>) -> &<$crate::runtime::SequencerRegistry<S> as ::sov_modules_api::Genesis>::Config {
                &config.sequencer_registry
            }
        }

        impl<S: ::sov_modules_api::Spec> GenesisConfig<S> {
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
                    chain_state: minimal_config.chain_state,
                    blob_storage: minimal_config.blob_storage,
                    prover_incentives: minimal_config.prover_incentives,
                    attester_incentives: minimal_config.attester_incentives,
                    $(
                        $module_name,
                    )*
                }
            }
        }

        impl<S: ::sov_modules_api::Spec> GenesisConfig<S>
        where <S::InnerZkvm as ::sov_modules_api::Zkvm>::CodeCommitment: Default,
         <S::OuterZkvm as ::sov_modules_api::Zkvm>::CodeCommitment: Default,{
            #[allow(unused)]
            /// Creates a [`$crate::runtime::GenesisParams`] from a [`GenesisConfig`].
            pub fn into_genesis_params(self) -> $crate::runtime::GenesisParams<Self> {
                $crate::runtime::GenesisParams {
                    runtime: self,
                }
            }
        }

        $(
            $crate::impl_runtime_hook!($id<S>, $hook);
        )*

        impl<S> $crate::runtime::Runtime<S> for $id<S> where
            S: ::sov_modules_api::Spec,
            $($runtime_trait_impl_bounds)*
        {
            type GenesisConfig = <Self as ::sov_modules_api::Genesis>::Config;

            type GenesisPaths = ();

            fn endpoints(_api_state: sov_modules_api::rest::ApiState<S>) -> $crate::runtime::RuntimeEndpoints {
                unimplemented!()
            }

            fn genesis_config(_genesis_paths: &Self::GenesisPaths) -> anyhow::Result<Self::GenesisConfig> {
                unimplemented!()
            }

        }

        impl<S> ::sov_modules_api::capabilities::HasCapabilities<S> for $id<S> where
            S: ::sov_modules_api::Spec,
        {
            type Capabilities<'a> = $crate::runtime::StandardProvenRollupCapabilities<'a, S>;

            type AuthorizationData = ::sov_modules_api::capabilities::AuthorizationData<S>;

            fn capabilities(&self) -> ::sov_modules_api::capabilities::Guard<Self::Capabilities<'_>> {
                ::sov_modules_api::capabilities::Guard::new(
                    $crate::runtime::StandardProvenRollupCapabilities {
                        bank: &self.bank,
                        gas_payer: &self.bank,
                        sequencer_registry: &self.sequencer_registry,
                        accounts: &self.accounts,
                        nonces: &self.nonces,
                        prover_incentives: &self.prover_incentives,
                        attester_incentives: &self.attester_incentives,
                    }
                )
            }

            $(
                fn gas_enforcer(&self) -> impl ::sov_modules_api::capabilities::GasEnforcer<S> {
                    self. $gas_enforcer_override_fn ()
                }
            )?
        }

        impl<S> sov_modules_api::capabilities::HasKernel<S> for $id<S> where
            S: ::sov_modules_api::Spec,
        {
            type BlobType = sov_modules_api::BlobDataWithId;
            type Kernel<'a> = $kernel_type;

            fn inner(&self) -> sov_modules_api::capabilities::Guard<Self::Kernel<'_>> {
                sov_modules_api::capabilities::Guard::new(Self::Kernel {
                    chain_state: &self.chain_state,
                    blob_storage: &self.blob_storage,
                })
            }

            fn kernel_with_slot_mapping(&self) -> std::sync::Arc<dyn ::sov_modules_api::capabilities::KernelWithSlotMapping<S>> {
                ::std::sync::Arc::new(self.chain_state.clone())
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

        $crate::impl_standard_runtime_authenticator!($id<S>);
    };
}

/// Implements a default `TransactionAuthenticator` that uses sov modules authentication.
#[macro_export]
macro_rules! impl_standard_runtime_authenticator {
    ($runtime:ty) => {
        /// The input for the runtime's authenticator functionality.
        #[derive(std::fmt::Debug, Clone, ::borsh::BorshDeserialize, ::borsh::BorshSerialize)]
        pub struct AuthenticatorInput(::sov_modules_api::RawTx);

        impl<S> ::sov_modules_api::capabilities::TransactionAuthenticator<S> for $runtime
        where
            S: ::sov_modules_api::Spec,
        {
            type Decodable = <$runtime as ::sov_modules_api::DispatchCall>::Decodable;
            type AuthorizationData = ::sov_modules_api::capabilities::AuthorizationData<S>;
            type Input = AuthenticatorInput;


            fn authenticate<Accessor: ::sov_modules_api::ProvableStateReader<::sov_state::User, Spec = S>>(
                &self,
                tx: &AuthenticatorInput,
                pre_exec_ws: &mut Accessor,
            ) -> ::core::result::Result<
                ::sov_modules_api::capabilities::AuthenticationOutput<
                    S,
                    Self::Decodable,
                    Self::AuthorizationData,
                >,
                ::sov_modules_api::capabilities::AuthenticationError,
            > {
                ::sov_modules_api::capabilities::authenticate::<_, S, Self>(
                    &tx.0.data,
                    pre_exec_ws,
                )
            }

            fn authenticate_unregistered<Accessor: ::sov_modules_api::ProvableStateReader<::sov_state::User, Spec = S>>(
                &self,
                tx: &AuthenticatorInput,
                pre_exec_ws: &mut Accessor,
            ) -> ::core::result::Result<
                ::sov_modules_api::capabilities::AuthenticationOutput<
                    S,
                    Self::Decodable,
                    Self::AuthorizationData,
                >,
                ::sov_modules_api::capabilities::UnregisteredAuthenticationError,
            > {
                ::sov_modules_api::capabilities::authenticate::<
                    _,
                    S,
                    Self
                >(&tx.0.data, pre_exec_ws) .map_err(|e| match e {
                    ::sov_modules_api::capabilities::AuthenticationError::FatalError(err, hash) => {
                        ::sov_modules_api::capabilities::UnregisteredAuthenticationError::FatalError(err, hash)
                    }
                    ::sov_modules_api::capabilities::AuthenticationError::OutOfGas(err) => {
                        ::sov_modules_api::capabilities::UnregisteredAuthenticationError::OutOfGas(err)
                    }
                })
            }

            fn add_standard_auth(tx: ::sov_modules_api::RawTx) -> Self::Input {
                AuthenticatorInput(tx)
            }
        }
    };
}

/// Generates a optimistic runtime containing the [`Bank`](sov_bank::Bank), [`AttesterIncentives`](sov_attester_incentives::AttesterIncentives),
/// and [`SequencerRegistry`](sov_sequencer_registry::SequencerRegistry) modules in addition to any provided as arguments. The runtime implements a basic kernel.
#[macro_export]
macro_rules! generate_optimistic_runtime {
    ($id:ident <= $($module_name:ident : $module_ty:path),*) => {
        $crate::generate_optimistic_runtime_with_kernel! {
            $id <= kernel_type: $crate::runtime::BasicKernel<'a, S>, $($module_name : $module_ty),*
        }
    };
}

/// Generates a optimistic runtime containing the [`Bank`](sov_bank::Bank), [`AttesterIncentives`](sov_attester_incentives::AttesterIncentives),
/// and [`SequencerRegistry`](sov_sequencer_registry::SequencerRegistry) modules in addition to any provided as arguments. The runtime implements a custom kernel.
#[macro_export]
macro_rules! generate_optimistic_runtime_with_kernel {
    ($id:ident <= kernel_type: $kernel_ty:ty, $($module_name:ident : $module_ty:path),*) => {
        $crate::generate_runtime! {
            name: $id,
            modules: [$($module_name : $module_ty),*],
            operating_mode: sov_modules_api::runtime::OperatingMode::Optimistic,
            minimal_genesis_config_type: $crate::runtime::genesis::optimistic::config::MinimalOptimisticGenesisConfig<S>,
            impl_hooks: [SlotHooks, KernelSlotHooks, FinalizeHook, ApplyBatchHooks, TxHooks],
            runtime_trait_impl_bounds: [],
            kernel_type: $kernel_ty,
        }
    };
}

/// Generates a zk runtime containing the [`Bank`](sov_bank::Bank), [`ProverIncentives`](sov_prover_incentives::ProverIncentives),
/// and [`SequencerRegistry`](sov_sequencer_registry::SequencerRegistry) modules in addition to any provided as arguments. The runtime implements a basic kernel.
#[macro_export]
macro_rules! generate_zk_runtime {
    ($id:ident <= $($module_name:ident : $module_ty:path),*) => {
        $crate::generate_zk_runtime_with_kernel! {
            kernel_type: $crate::runtime::BasicKernel<'a, S>,
            $id <= $($module_name : $module_ty),*
        }
    };
}

/// Generates a zk runtime containing the [`Bank`](sov_bank::Bank), [`ProverIncentives`](sov_prover_incentives::ProverIncentives),
/// and [`SequencerRegistry`](sov_sequencer_registry::SequencerRegistry) modules in addition to any provided as arguments. The runtime implements a custom kernel.
#[macro_export]
macro_rules! generate_zk_runtime_with_kernel {
    (kernel_type: $kernel_ty:ty, $id:ident <= $($module_name:ident : $module_ty:path),*) => {
        $crate::generate_runtime! {
            name: $id,
            modules: [$($module_name : $module_ty),*],
            operating_mode: sov_modules_api::runtime::OperatingMode::Zk,
            minimal_genesis_config_type: $crate::runtime::genesis::zk::MinimalZkGenesisConfig<S>,
            impl_hooks: [SlotHooks, KernelSlotHooks, FinalizeHook, ApplyBatchHooks, TxHooks],
            runtime_trait_impl_bounds: [],
            kernel_type: $kernel_ty
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
