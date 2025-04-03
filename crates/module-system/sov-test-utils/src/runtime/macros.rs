/// Generates a bare runtime without implementing the `HasCapabilities` trait.
#[macro_export]
macro_rules! generate_bare_runtime_without_capabilities {
    (
        name: $id:ident,
        modules: [$($module_name:ident : $module_ty:path),* $(,)?],
        operating_mode: $operating_mode:path,
        minimal_genesis_config_type: $minimal_genesis_config_ty:path,
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
            ::sov_modules_api::Hooks,
            ::sov_modules_api::DispatchCall,
            ::sov_modules_api::Event,
            ::sov_modules_api::MessageCodec,
            ::sov_modules_api::macros::CliWallet,
            ::sov_modules_api::macros::RuntimeRestApi,
        )]
        pub struct $id<S: ::sov_modules_api::Spec>  where
        $($runtime_trait_impl_bounds)*
        {
            /// The sequencer registry module.
            pub sequencer_registry: $crate::runtime::SequencerRegistry<S>,
            /// The bank module.
            pub bank: $crate::runtime::Bank<S>,
            /// The accounts module
            pub accounts: $crate::runtime::Accounts<S>,
            /// The uniqueness module
            pub uniqueness: $crate::runtime::Uniqueness<S>,
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

        impl<S: ::sov_modules_api::Spec> $crate::runtime::traits::MinimalGenesis<S> for $id<S>
            where
            $($runtime_trait_impl_bounds)*
        {
            fn sequencer_registry_config(config: &GenesisConfig<S>) -> &<$crate::runtime::SequencerRegistry<S> as ::sov_modules_api::Genesis>::Config {
                &config.sequencer_registry
            }
        }

        impl<S: ::sov_modules_api::Spec> GenesisConfig<S> where
            $($runtime_trait_impl_bounds)*
        {
            #[allow(unused)]
            /// Creates a new [`GenesisConfig`] from a minimal genesis config [`::sov_modules_api::Genesis::Config`].
            pub fn from_minimal_config(minimal_config: $minimal_genesis_config_ty,
                $($module_name: <$module_ty as ::sov_modules_api::Genesis>::Config),*
            ) -> Self {
                Self {
                    sequencer_registry: minimal_config.sequencer_registry,
                    bank: minimal_config.bank,
                    accounts: minimal_config.accounts,
                    uniqueness: minimal_config.uniqueness,
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
        where
            $($runtime_trait_impl_bounds)*
        {
            #[allow(unused)]
            /// Creates a [`$crate::runtime::GenesisParams`] from a [`GenesisConfig`].
            pub fn into_genesis_params(self) -> $crate::runtime::GenesisParams<Self> {
                $crate::runtime::GenesisParams {
                    runtime: self,
                }
            }
        }

        impl<S> $crate::runtime::Runtime<S> for $id<S> where
            S: ::sov_modules_api::Spec,
            ::sov_modules_api::transaction::Transaction::<Self, S>: $crate::sov_universal_wallet::schema::SchemaGenerator,
            <Self as ::sov_modules_api::DispatchCall>::Decodable: $crate::sov_universal_wallet::schema::SchemaGenerator,
            $($runtime_trait_impl_bounds)*
        {
            const CHAIN_HASH: [u8; 32] = [11; 32];

            type GenesisConfig = <Self as ::sov_modules_api::Genesis>::Config;

            type GenesisInput = ();

            fn endpoints(api_state: sov_modules_api::rest::ApiState<S>) -> ::sov_modules_api::NodeEndpoints {
                use $crate::sov_rollup_apis::dedup::{DeDupEndpoint, NonceDeDupEndpoint};
                use $crate::sov_rollup_apis::schema::{SchemaEndpoint, StandardSchemaEndpoint};
                use $crate::sov_universal_wallet::schema::{ChainData, Schema};
                use ::sov_modules_api::macros::config_value;
                use ::sov_modules_api::transaction::{Transaction, UnsignedTransaction};
                use ::sov_modules_api::rest::HasRestApi;

                let axum_router = Self::default().rest_api(api_state.clone());
                // Provide an endpoint to return dedup information associated with addresses.
                // Since our runtime is using the uniqueness module we can use the provided `NonceDeDupEndpoint` implementation.
                let dedup_endpoint = NonceDeDupEndpoint::new(api_state.clone());
                let axum_router = axum_router.merge(dedup_endpoint.axum_router());

                let schema = Schema::of_rollup_types_with_chain_data::<
                Transaction<Self, S>,
                UnsignedTransaction<Self, S>,
                <Self as ::sov_modules_api::DispatchCall>::Decodable,
                S::Address,
                >(ChainData {
                    chain_id: config_value!("CHAIN_ID"),
                    chain_name: config_value!("CHAIN_NAME").to_string(),
                })
                .unwrap();

                let schema_endpoint = StandardSchemaEndpoint::new(
                    &schema
                )
                .expect("Failed to initialize StandardSchemaEndpoint");
                let axum_router = axum_router.merge(schema_endpoint.axum_router());

                ::sov_modules_api::NodeEndpoints {
                    axum_router,
                    jsonrpsee_module: ::sov_modules_api::prelude::jsonrpsee::RpcModule::new(()),
                    background_handles: Vec::new(),
                }
            }

            fn genesis_config(_input: &Self::GenesisInput) -> ::sov_modules_api::prelude::anyhow::Result<Self::GenesisConfig> {
                unimplemented!()
            }

            fn operating_mode(genesis: &Self::GenesisConfig) -> ::sov_modules_api::runtime::OperatingMode {
                genesis.chain_state.operating_mode
            }

        }


        impl<S> sov_modules_api::capabilities::HasKernel<S> for $id<S> where
            S: ::sov_modules_api::Spec,
            $($runtime_trait_impl_bounds)*
        {

            type Kernel<'a> = $kernel_type;

            fn inner(&mut self) -> sov_modules_api::capabilities::Guard<Self::Kernel<'_>> {
                sov_modules_api::capabilities::Guard::new(Self::Kernel {
                    chain_state: &mut self.chain_state,
                    blob_storage: &mut self.blob_storage,
                })
            }

            fn kernel_with_slot_mapping(&self) -> std::sync::Arc<dyn ::sov_modules_api::capabilities::KernelWithSlotMapping<S>> {
                ::std::sync::Arc::new(self.chain_state.clone())
            }
        }
    }
}

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
        gas_enforcer: $payer_name:ident : $gas_enforcer_ty:ty,
        runtime_trait_impl_bounds: [$($runtime_trait_impl_bounds:tt)*],
        kernel_type: $kernel_type:ty
        // optional final comma
        $(,)?
    ) => {
        $crate::generate_bare_runtime_without_capabilities! {
            name: $id,
            modules: [$($module_name : $module_ty),*],
            operating_mode: $operating_mode,
            minimal_genesis_config_type: $minimal_genesis_config_ty,
            runtime_trait_impl_bounds: [$($runtime_trait_impl_bounds)*],
            kernel_type: $kernel_type,
        }

        impl<S> ::sov_modules_api::capabilities::HasCapabilities<S> for $id<S> where
        S: ::sov_modules_api::Spec,
        $($runtime_trait_impl_bounds)*
        {
            type Capabilities<'a> = $crate::runtime::StandardProvenRollupCapabilities<'a, S, &'a mut $gas_enforcer_ty>;

            fn capabilities(&mut self) -> ::sov_modules_api::capabilities::Guard<Self::Capabilities<'_>> {
                ::sov_modules_api::capabilities::Guard::new(
                    $crate::runtime::StandardProvenRollupCapabilities {
                        bank: &mut self.bank,
                        gas_payer: &mut self. $payer_name,
                        sequencer_registry: &mut self.sequencer_registry,
                        accounts: &mut self.accounts,
                        uniqueness: &mut self.uniqueness,
                        prover_incentives: &mut self.prover_incentives,
                        attester_incentives: &mut self.attester_incentives,
                    }
                )
            }

        }
    };
    (
        name: $id:ident,
        modules: [$($module_name:ident : $module_ty:path),* $(,)?],
        operating_mode: $operating_mode:path,
        minimal_genesis_config_type: $minimal_genesis_config_ty:path,
        runtime_trait_impl_bounds: [$($runtime_trait_impl_bounds:tt)*],
        kernel_type: $kernel_type:ty
        // optional final comma
        $(,)?
    ) => {
        $crate::generate_bare_runtime_without_capabilities! {
            name: $id,
            modules: [$($module_name : $module_ty),*],
            operating_mode: $operating_mode,
            minimal_genesis_config_type: $minimal_genesis_config_ty,
            runtime_trait_impl_bounds: [$($runtime_trait_impl_bounds)*],
            kernel_type: $kernel_type,
        }

        impl<S> ::sov_modules_api::capabilities::HasCapabilities<S> for $id<S> where
        S: ::sov_modules_api::Spec,
        $($runtime_trait_impl_bounds)*
        {
            type Capabilities<'a> = $crate::runtime::StandardProvenRollupCapabilities<'a, S>;

            fn capabilities(&mut self) -> ::sov_modules_api::capabilities::Guard<Self::Capabilities<'_>> {
                ::sov_modules_api::capabilities::Guard::new(
                    $crate::runtime::StandardProvenRollupCapabilities {
                        bank: &mut self.bank,
                        gas_payer: (),
                        sequencer_registry: &mut self.sequencer_registry,
                        accounts: &mut self.accounts,
                        uniqueness: &mut self.uniqueness,
                        prover_incentives: &mut self.prover_incentives,
                        attester_incentives: &mut self.attester_incentives,
                    }
                )
            }

        }
    }
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
            ::sov_modules_api::transaction::Transaction::<Self, S>: $crate::sov_universal_wallet::schema::SchemaGenerator,
            ::sov_modules_api::transaction::UnsignedTransaction::<Self, S>: $crate::sov_universal_wallet::schema::SchemaGenerator,
            <Self as ::sov_modules_api::DispatchCall>::Decodable: $crate::sov_universal_wallet::schema::SchemaGenerator,
        {
            type Decodable = <$runtime as ::sov_modules_api::DispatchCall>::Decodable;
            type Input = AuthenticatorInput;

            type Signature = ::sov_modules_api::transaction::TransactionWithoutCall<S>;

            fn decode_serialized_tx(
                &self,
                tx: &sov_modules_api::FullyBakedTx,
            ) -> Result<(Self::Decodable, Self::Signature), ::sov_modules_api::capabilities::FatalError> {
                let tx: AuthenticatorInput = borsh::from_slice(&tx.data).map_err(|e| {
                    sov_modules_api::capabilities::FatalError::DeserializationFailed(e.to_string())
                })?;
                ::sov_modules_api::capabilities::decode_sov_tx::<_, Self>(&tx.0.data)
            }


            fn authenticate<Accessor: ::sov_modules_api::ProvableStateReader<::sov_state::User, Spec = S>>(
                &self,
                tx: &sov_modules_api::FullyBakedTx,
                pre_exec_ws: &mut Accessor,
            ) -> ::core::result::Result<
                ::sov_modules_api::capabilities::AuthenticationOutput<
                    S,
                    Self::Decodable,
                >,
                ::sov_modules_api::capabilities::AuthenticationError,
            > {
                let input: AuthenticatorInput = borsh::from_slice(&tx.data).map_err(|e| {
                    sov_modules_api::capabilities::fatal_deserialization_error::<_, S, _>(
                        &tx.data, e, pre_exec_ws,
                    )
                })?;

                ::sov_modules_api::capabilities::authenticate::<_, S, Self>(
                    &input.0.data,
                    &<$runtime as $crate::runtime::Runtime<S>>::CHAIN_HASH,
                    pre_exec_ws,
                )
            }

            fn authenticate_unregistered<Accessor: ::sov_modules_api::ProvableStateReader<::sov_state::User, Spec = S>>(
                &self,
                batch: &sov_modules_api::capabilities::BatchFromUnregisteredSequencer,
                pre_exec_ws: &mut Accessor,
            ) -> ::core::result::Result<
                ::sov_modules_api::capabilities::AuthenticationOutput<
                    S,
                    Self::Decodable,
                >,
                ::sov_modules_api::capabilities::UnregisteredAuthenticationError,
            > {
                ::sov_modules_api::capabilities::authenticate::<
                    _,
                    S,
                    Self
                >(
                    &batch.tx.data,
                    &<$runtime as $crate::runtime::Runtime<S>>::CHAIN_HASH,
                    pre_exec_ws
                ) .map_err(|e| match e {
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
