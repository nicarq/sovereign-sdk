//! Traits and utilities for writing REST(ful) APIs that expose rollup data.
//!
//! Sovereign rollup nodes can expose their data over HTTP in two ways:
//!
//! 1. A JSON-RPC interface. This is mostly intended for cross-compatbility with
//!    third-party JSON-RPC interfaces e.g. the Ethereum JSON-RPC API.
//! 2. An [`axum`]-based REST API. Axum is the most flexible and capable
//!    solution out of the two listed here, and integrates most easily with the
//!    rest of the Sovereign SDK ecosystem.
//!
//! This Rust module is exclusively concerned with the latter case, and exposes
//! all the necessary tools for composing Axum routers within the rollup node.
//! If you're looking for JSON-RPC API documentation, please refer to
//! [`crate::macros::expose_rpc`].
//!
//! Nodes expose rollup data using a combination of three traits:
//!
//! | Trait | Derivable | Implemented by |
//! | ----- | --------- | ---- |
//! | [`HasRestApi`] | With [`ModuleRestApi`] and [`RuntimeRestApi`] | Modules and runtimes |
//! | [`HasCustomRestApi`]   | ❌ | Modules and runtimes |
//!
//! Implementing or deriving *any* of these traits is optional, but types
//! implementing [`HasCustomRestApi`] ought to also derive [`HasRestApi`], or
//! no REST API will be available for them.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use serde::{Deserialize, Serialize};
use sov_rest_utils::{PathWithErrorHandling, QueryStringValidation, ValidatedQuery};
use sov_state::namespaces::CompileTimeNamespace;
use sov_state::{StateCodec, StateItemCodec};

use crate::hooks::TxHooks;
use crate::map::NamespacedStateMap;
use crate::value::NamespacedStateValue;
use crate::vec::NamespacedStateVec;
use crate::{Module, ModuleId, ModuleInfo, Spec, StateReaderAndWriter, WorkingSet};

/// A [`tokio::sync::watch::Receiver`] for a [`Spec`]'s storage.
pub type StorageReceiver<S> = tokio::sync::watch::Receiver<<S as Spec>::Storage>;

pub use sov_modules_macros::{ModuleRestApi, RuntimeRestApi};

/// Utilities for building opinionated REST(ful) APIs with [`axum`].
#[doc(inline)]
pub extern crate sov_rest_utils as utils;

/// This trait is intended to be derived via
/// [`crate::macros::ModuleRestApi`] by modules and via
/// [`crate::macros::RuntimeRestApi`] by runtimes.
/// runtimes, and it provides a fair amount of paths and general information
/// about the rollup, including but not limited to:
///
/// - A list of all modules included in the runtime, and related metadata.
/// - Module state, both latest and historical; per-variable state overview;
///   pagination for structured state items like
///   [`StateVec`](crate::containers::StateVec).
pub trait HasRestApi<S: Spec> {
    fn rest_api(&self, storage: StorageReceiver<S>) -> axum::Router<()>;
}

/// Makes deriving [`HasRestApi`] for modules optional, with the autoref trick.
impl<M: ModuleInfo> HasRestApi<M::Spec> for &M {
    fn rest_api(&self, _storage: StorageReceiver<M::Spec>) -> axum::Router<()> {
        axum::Router::new()
    }
}

/// Optionally exposes hand-written, custom API routes for a module or
/// runtime.
///
/// This trait cannot be derived, and implementing it is entirely optional.
/// A module that implements this trait will be automatically exposed as part of
/// the runtime API, as [`ModuleRestApi`] will automatically merge the two.
///
/// # Example
///
/// ```
/// use sov_modules_api::prelude::*;
/// use sov_modules_api::{ModuleId, ModuleInfo, StateValue};
/// use sov_modules_api::rest::{HasCustomRestApi, StorageReceiver};
///
/// #[derive(Clone, ModuleInfo, ModuleRestApi)]
/// struct MyModule<S: Spec> {
///     #[id]
///     id: ModuleId,
///     #[state]
///     state_item: StateValue<S::Address>,
/// }
///
/// impl<S: Spec> HasCustomRestApi<S> for MyModule<S> {
///     fn custom_rest_api(&self, storage: StorageReceiver<S>) -> axum::Router<()> {
///         use axum::routing::get;
///
///         axum::Router::new()
///             .route("/", get(|| async { "Hello, world!" }))
///             .with_state(self.clone())
///     }
/// }
/// # // BEGIN MODULE IMPL, COPY-PASTE-ME (https://doc.rust-lang.org/rustdoc/write-documentation/documentation-tests.html#hiding-portions-of-the-example)
/// # impl<S: Spec> sov_modules_api::Module for MyModule<S> {
/// #    type Spec = S;
/// #    type Config = ();
/// #    type CallMessage = ();
/// #    type Event = ();
/// #
/// #    fn genesis(
/// #        &self,
/// #        _config: &Self::Config,
/// #        _working_set: &mut impl sov_modules_api::state::GenesisState<S>,
/// #    ) -> Result<(), sov_modules_api::Error> {
/// #        Ok(())
/// #    }
/// #
/// #    fn call(
/// #        &self,
/// #        _msg: Self::CallMessage,
/// #        _context: &Context<Self::Spec>,
/// #        _working_set: &mut impl sov_modules_api::state::TxState<S>,
/// #    ) -> Result<sov_modules_api::CallResponse, sov_modules_api::Error> {
/// #        unimplemented!()
/// #    }
/// # }
/// # // END MODULE IMPL
/// ```
pub trait HasCustomRestApi<S: Spec> {
    fn custom_rest_api(&self, storage: StorageReceiver<S>) -> axum::Router<()>;

    /// Returns the OpenAPI specification for [`HasCustomRestApi::custom_rest_api`].
    /// [`None`] means there is no known OpenAPI spec for the API.
    fn openapi_spec(&self) -> Option<serde_json::Value> {
        None
    }
}

/// In case [`HasCustomRestApi`] is implemented for a [`Module`] or Runtime, an
/// empty [`axum::Router`] will be returned instead.
///
/// This "blanket" implementation uses the autoref trick.
impl<T, S: Spec> HasCustomRestApi<S> for &T {
    fn custom_rest_api(&self, _storage: StorageReceiver<S>) -> axum::Router<()> {
        axum::Router::new()
    }
}

/// A wrapper around [`Spec::Storage`] that is appropriate for use as a state
/// type of module and runtime [`axum::Router`]s.
pub struct ModuleRestApiState<S: Spec> {
    storage_receiver: StorageReceiver<S>,
}

impl<S> ModuleRestApiState<S>
where
    S: Spec,
{
    /// Creates a [`ModuleRestApiState`] that subscribes to the given
    /// [`StorageReceiver`].
    pub fn new(storage_receiver: StorageReceiver<S>) -> Self {
        Self { storage_receiver }
    }

    /// Returns a reference to the latest available storage.
    ///
    /// You will usually not call this method directly, as storage alone
    /// can't be used to read state.
    pub fn storage(&self) -> impl Deref<Target = S::Storage> + '_ {
        self.storage_receiver.borrow()
    }

    pub fn working_set(&self) -> WorkingSet<S> {
        let storage = self.storage().clone();
        WorkingSet::new(storage)
    }
}

/// This Rust module is **NOT** part of the public API of the crate, and can
/// change at any time.
#[doc(hidden)]
pub mod __macros_private {
    pub use base_impls::*;
    pub use state_item::*;

    use super::*;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StateItemInfo {
        pub r#type: StateItemKind,
        #[serde(skip)]
        pub name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        pub namespace: Namespace,
        pub prefix: Prefix,
    }

    fn maybe_archival_ws<S: Spec>(
        working_set: WorkingSet<S>,
        height_opt: Option<ValidatedQuery<HeightQueryParam>>,
    ) -> WorkingSet<S> {
        if let Some(ValidatedQuery(height)) = height_opt {
            working_set.get_archival_at(height.height)
        } else {
            working_set
        }
    }

    #[derive(Copy, Clone, Debug, Deserialize)]
    pub struct HeightQueryParam {
        height: u64,
    }

    impl QueryStringValidation for HeightQueryParam {}

    pub trait GetStateItemInfo {
        const STATE_ITEM_KIND: StateItemKind;
        const NAMESPACE: sov_state::Namespace;
    }

    impl<N, V, Codec> GetStateItemInfo for NamespacedStateValue<N, V, Codec>
    where
        N: CompileTimeNamespace,
    {
        const STATE_ITEM_KIND: StateItemKind = StateItemKind::StateValue;
        const NAMESPACE: sov_state::Namespace = N::NAMESPACE;
    }

    impl<N, V, Codec> GetStateItemInfo for NamespacedStateVec<N, V, Codec>
    where
        N: CompileTimeNamespace,
    {
        const STATE_ITEM_KIND: StateItemKind = StateItemKind::StateVec;
        const NAMESPACE: sov_state::Namespace = N::NAMESPACE;
    }

    impl<N, K, V, Codec> GetStateItemInfo for NamespacedStateMap<N, K, V, Codec>
    where
        N: CompileTimeNamespace,
    {
        const STATE_ITEM_KIND: StateItemKind = StateItemKind::StateMap;
        const NAMESPACE: sov_state::Namespace = N::NAMESPACE;
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RuntimeObject {
        //modules: Vec<ModuleObject>,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct Prefix(pub sov_state::Prefix);

    impl Serialize for Prefix {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let s = format!("0x{}", hex::encode(&self.0));
            serializer.serialize_str(&s)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", tag = "type", rename = "module")]
    pub struct ModuleObject {
        pub id: ModuleId,
        pub name: String,
        pub description: Option<String>,
        pub prefix: Prefix,
        pub state_items: HashMap<String, StateItemInfo>,
    }

    impl ModuleObject {
        pub fn new(
            module: &(impl ModuleInfo + ?Sized),
            description: Option<String>,
            state_items: HashMap<String, StateItemInfo>,
        ) -> Self {
            Self {
                id: *module.id(),
                description,
                name: module.prefix().module_name().to_owned(),
                prefix: Prefix(module.prefix().into()),
                state_items,
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum StateItemKind {
        StateValue,
        StateVec,
        StateMap,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum StateItemMetadata<T> {
        StateValue { value: Option<T> },
        StateVec { length: usize },
        StateMap,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Namespace {
        User,
        Kernel,
        Accessory,
    }

    impl From<sov_state::namespaces::Namespace> for Namespace {
        fn from(value: sov_state::namespaces::Namespace) -> Self {
            match value {
                sov_state::namespaces::Namespace::User => Self::User,
                sov_state::namespaces::Namespace::Kernel => Self::Kernel,
                sov_state::namespaces::Namespace::Accessory => Self::Accessory,
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StateItemObject<T> {
        pub r#type: StateItemMetadata<T>,
        pub prefix: Prefix,
        pub name: String,
        pub version: StateItemVersion,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        pub namespace: Namespace,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StateVecItem<T> {
        pub index: usize,
        pub value: T,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StateMapItem<K, V> {
        pub key: K,
        pub value: V,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum StateItemVersion {
        Latest,
        #[serde(untagged)]
        Height(u64),
    }

    impl StateItemVersion {
        pub fn from_query_param(param_opt: Option<ValidatedQuery<HeightQueryParam>>) -> Self {
            match param_opt {
                None => Self::Latest,
                Some(ValidatedQuery(height)) => Self::Height(height.height),
            }
        }
    }

    /// Trait "alias" for simpler trait bounds.
    pub trait ModuleSendSync: Module + Send + Sync + 'static {}
    impl<M> ModuleSendSync for M where M: Module + Send + Sync + 'static {}

    pub mod state_item {
        //! When autogenerating APIs for state items, we have a few
        //! requirements:
        //!
        //! 1. We require a "blanket" implementation on **ALL** state items,
        //!    even the ones that for which we can't provide an API, for
        //!    the autoref trick. Without this, compilation would fail unless
        //!    all unsuitable state items were marked with `skip`. See
        //!    [`StateItemRestApi`].
        //! 2. We require a method that is purposefully **NOT**
        //!    blanket-implemented, to cause a compilation errors when unsuitable
        //!    state items are marked with `include`. See
        //!    [`StateItemRestApiExists`].

        use std::marker::PhantomData;

        use sov_rest_utils::{errors, ApiResult};

        use super::*;
        use crate::StateReader;

        #[derive(derivative::Derivative)]
        #[derivative(Clone(bound = ""))]
        pub struct StateItemRestApiImpl<M: Module, T> {
            pub storage: StorageReceiver<M::Spec>,
            pub state_item_info: StateItemInfo,
            pub phantom: PhantomData<T>,
        }

        pub trait StateItemRestApi {
            fn state_item_rest_api(&self) -> axum::Router<()>;
        }

        // "Blanket" implementation for all "unsuitable" state items e.g.
        // non-`serde` compatible ones, using the autoref trick.
        impl<T> StateItemRestApi for &T {
            fn state_item_rest_api(&self) -> axum::Router<()> {
                axum::Router::new()
            }
        }

        /// It's important for this trait to require [`StateItemRestApi`], otherwise type
        /// bounds might get out of sync.
        pub trait StateItemRestApiExists: StateItemRestApi {
            /// By calling this method, the proc-macro can "assert" that the type
            /// actually implements [`StateItemRestApi`].
            fn exists(&self) {}
        }

        impl<N, M, T, Codec> StateItemRestApiImpl<M, NamespacedStateValue<N, T, Codec>>
        where
            N: CompileTimeNamespace,
            M: ModuleSendSync,
            WorkingSet<M::Spec>: StateReader<N>,
            T: Serialize,
            Codec: StateCodec,
            Codec::ValueCodec: StateItemCodec<T>,
        {
            async fn get_state_value_route(
                State(state): State<Self>,
                height_opt: Option<ValidatedQuery<HeightQueryParam>>,
            ) -> ApiResult<StateItemObject<T>> {
                let mut working_set = maybe_archival_ws(
                    WorkingSet::<M::Spec>::new(state.storage.borrow().clone()),
                    height_opt,
                );

                let state_value = NamespacedStateValue::<N, T, Codec>::with_codec(
                    state.state_item_info.prefix.0.clone(),
                    Codec::default(),
                );

                let read_value: Option<T> = state_value.get(&mut working_set);
                Ok(StateItemObject {
                    r#type: StateItemMetadata::StateValue { value: read_value },
                    prefix: state.state_item_info.prefix,
                    name: state.state_item_info.name.clone(),
                    description: state.state_item_info.description.clone(),
                    version: StateItemVersion::from_query_param(height_opt),
                    namespace: state.state_item_info.namespace.clone(),
                }
                .into())
            }
        }

        impl<N, M, T, Codec> StateItemRestApi for StateItemRestApiImpl<M, NamespacedStateValue<N, T, Codec>>
        where
            N: CompileTimeNamespace,
            M: ModuleSendSync,
            WorkingSet<M::Spec>: StateReader<N>,
            T: Serialize + Send + Sync + 'static,
            Codec: StateCodec,
            Codec::ValueCodec: StateItemCodec<T>,
        {
            fn state_item_rest_api(&self) -> axum::Router<()> {
                axum::Router::new()
                    .route("/", get(Self::get_state_value_route))
                    .with_state(self.clone())
            }
        }

        impl<N, M, T, Codec> StateItemRestApiImpl<M, NamespacedStateVec<N, T, Codec>>
        where
            N: CompileTimeNamespace,
            M: ModuleSendSync,
            WorkingSet<M::Spec>: StateReader<N> + StateReaderAndWriter<N>,
            T: Serialize,
            Codec: StateCodec,
            Codec::KeyCodec: StateItemCodec<usize>,
            Codec::ValueCodec: StateItemCodec<T> + StateItemCodec<usize>,
        {
            fn working_set_and_vec(
                &self,
                height_opt: Option<ValidatedQuery<HeightQueryParam>>,
            ) -> (WorkingSet<M::Spec>, NamespacedStateVec<N, T, Codec>) {
                (
                    maybe_archival_ws(WorkingSet::new(self.storage.borrow().clone()), height_opt),
                    NamespacedStateVec::with_codec(
                        self.state_item_info.prefix.0.clone(),
                        Codec::default(),
                    ),
                )
            }

            async fn get_state_vec_route(
                State(state): State<Self>,
                height_opt: Option<ValidatedQuery<HeightQueryParam>>,
            ) -> ApiResult<StateItemObject<T>> {
                let (mut working_set, state_vec) = Self::working_set_and_vec(&state, height_opt);

                let length = state_vec.len(&mut working_set);
                Ok(StateItemObject {
                    r#type: StateItemMetadata::<T>::StateVec { length },
                    prefix: state.state_item_info.prefix,
                    version: StateItemVersion::from_query_param(height_opt),
                    name: state.state_item_info.name.clone(),
                    description: state.state_item_info.description.clone(),
                    namespace: state.state_item_info.namespace.clone(),
                }
                .into())
            }

            async fn get_state_vec_item_route(
                State(state): State<Self>,
                PathWithErrorHandling(item_index): PathWithErrorHandling<usize>,
                height_opt: Option<ValidatedQuery<HeightQueryParam>>,
            ) -> ApiResult<StateVecItem<T>> {
                let (mut working_set, state_vec) = Self::working_set_and_vec(&state, height_opt);

                let read_value = state_vec
                    .get(item_index, &mut working_set)
                    .ok_or_else(|| errors::not_found_404("StateVecItem", item_index.to_string()))?;

                Ok(StateVecItem {
                    index: item_index,
                    value: read_value,
                }
                .into())
            }
        }

        impl<N, M, T, Codec> StateItemRestApi for StateItemRestApiImpl<M, NamespacedStateVec<N, T, Codec>>
        where
            N: CompileTimeNamespace,
            M: ModuleSendSync,
            WorkingSet<M::Spec>: StateReader<N> + StateReaderAndWriter<N>,
            T: Serialize + Clone + Send + Sync + 'static,
            Codec: StateCodec,
            Codec::KeyCodec: StateItemCodec<usize>,
            Codec::ValueCodec: StateItemCodec<T> + StateItemCodec<usize>,
        {
            fn state_item_rest_api(&self) -> axum::Router<()> {
                axum::Router::new()
                    .route("/", get(Self::get_state_vec_route))
                    .route("/items/:index/", get(Self::get_state_vec_item_route))
                    .with_state(self.clone())
            }
        }

        impl<N, M, K, V, Codec> StateItemRestApiImpl<M, NamespacedStateMap<N, K, V, Codec>>
        where
            N: CompileTimeNamespace,
            M: ModuleSendSync,
            WorkingSet<M::Spec>: StateReader<N>,
            K: Serialize + serde::de::DeserializeOwned,
            V: Serialize,
            Codec: StateCodec,
            Codec::KeyCodec: StateItemCodec<K>,
            Codec::ValueCodec: StateItemCodec<V>,
        {
            async fn get_state_map_route(State(state): State<Self>) -> ApiResult<StateItemInfo> {
                Ok(StateItemInfo {
                    r#type: StateItemKind::StateMap,
                    prefix: state.state_item_info.prefix,
                    description: state.state_item_info.description.clone(),
                    name: state.state_item_info.name.clone(),
                    namespace: state.state_item_info.namespace.clone(),
                }
                .into())
            }

            async fn get_state_map_item_route(
                State(state): State<Self>,
                PathWithErrorHandling(key): PathWithErrorHandling<K>,
                height_opt: Option<ValidatedQuery<HeightQueryParam>>,
            ) -> ApiResult<StateMapItem<K, V>> {
                let mut working_set = maybe_archival_ws(
                    WorkingSet::<M::Spec>::new(state.storage.borrow().clone()),
                    height_opt,
                );
                let state_map = NamespacedStateMap::<N, K, V, Codec>::with_codec(
                    state.state_item_info.prefix.0.clone(),
                    Codec::default(),
                );

                let read_value = state_map.get(&key, &mut working_set).ok_or_else(|| {
                    errors::not_found_404(
                        "StateMapItem",
                        serde_json::to_string(&key).unwrap_or_else(|_| "unknown".to_string()),
                    )
                })?;

                Ok(StateMapItem {
                    key,
                    value: read_value,
                }
                .into())
            }
        }

        impl<N, M, K, V, Codec> StateItemRestApi
            for StateItemRestApiImpl<M, NamespacedStateMap<N, K, V, Codec>>
        where
            N: CompileTimeNamespace,
            M: ModuleSendSync,
            WorkingSet<M::Spec>: StateReader<N> + StateReaderAndWriter<N>,
            K: Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
            V: Serialize + Clone + Send + Sync + 'static,
            Codec: StateCodec,
            Codec::KeyCodec: StateItemCodec<K>,
            Codec::ValueCodec: StateItemCodec<V>,
        {
            fn state_item_rest_api(&self) -> axum::Router<()> {
                axum::Router::new()
                    .route("/", get(Self::get_state_map_route))
                    .route("/items/:key", get(Self::get_state_map_item_route))
                    .with_state(self.clone())
            }
        }

        impl<N, M, T, Codec> StateItemRestApiExists
            for StateItemRestApiImpl<M, NamespacedStateValue<N, T, Codec>>
        where
            M: Module,
            Self: StateItemRestApi,
        {
        }

        impl<N, M, T, Codec> StateItemRestApiExists
            for StateItemRestApiImpl<M, NamespacedStateVec<N, T, Codec>>
        where
            M: Module,
            Self: StateItemRestApi,
        {
        }

        impl<N, M, K, V, Codec> StateItemRestApiExists
            for StateItemRestApiImpl<M, NamespacedStateMap<N, K, V, Codec>>
        where
            M: Module,
            Self: StateItemRestApi,
        {
        }
    }

    pub mod base_impls {
        use sov_rest_utils::ApiResult;

        use super::*;

        /// A basic implementor of [`HasRestApi`] for a runtime.
        ///
        /// The resulting [`axum::Router`] should then be merged with runtime-specific
        /// routes, e.g. for each child module.
        ///
        /// We can safely assume that all runtimes implement [`TxHooks`], which
        /// happens to expose an associated type [`Spec`] and, as such, is a
        /// great way for a proc-macro to "get" the runtime's [`Spec`] without
        /// having to guess which generic parameter it is.
        #[derive(derivative::Derivative)]
        #[derivative(Clone(bound = ""))]
        pub struct RuntimeRestApiBaseImpl<R: TxHooks> {
            pub runtime: Arc<R>,
            // TODO(@neysofu): module listing.
        }

        impl<R> RuntimeRestApiBaseImpl<R>
        where
            R: TxHooks + Send + Sync + 'static,
        {
            async fn root_handler(State(_state): State<Self>) -> ApiResult<RuntimeObject> {
                Ok(RuntimeObject {}.into())
            }
        }

        impl<R> HasRestApi<R::Spec> for RuntimeRestApiBaseImpl<R>
        where
            R: TxHooks + Send + Sync + 'static,
        {
            fn rest_api(&self, _storage: StorageReceiver<R::Spec>) -> axum::Router<()> {
                axum::Router::new()
                    .route("/", get(Self::root_handler))
                    .with_state(self.clone())
                    .fallback(sov_rest_utils::errors::global_404)
            }
        }

        /// A basic implementor of [`HasRestApi`] for a module.
        ///
        /// The resulting [`axum::Router`] should then be merged with module-specific
        /// routes, e.g. for each state item.
        #[derive(Clone)]
        pub struct ModuleRestApiBaseImpl<M: Module> {
            pub module: Arc<M>,
            pub description: Option<String>,
            pub storage: StorageReceiver<M::Spec>,
            pub state_items: HashMap<String, StateItemInfo>,
        }

        impl<M> HasRestApi<<M as Module>::Spec> for ModuleRestApiBaseImpl<M>
        where
            M: ModuleSendSync + ModuleInfo + Clone,
        {
            fn rest_api(&self, _storage: StorageReceiver<<M as Module>::Spec>) -> axum::Router<()> {
                axum::Router::new()
                    .route("/", get(Self::root_route))
                    .with_state(self.clone())
            }
        }

        impl<M> ModuleRestApiBaseImpl<M>
        where
            M: ModuleSendSync + ModuleInfo + Clone,
        {
            /// The handler function for the root path of the router, which
            /// returns some general information about the module (name, ID,
            /// etc.).
            async fn root_route(State(state): State<Self>) -> ApiResult<ModuleObject> {
                Ok(ModuleObject::new(
                    &*state.module,
                    state.description.clone(),
                    state.state_items.clone(),
                )
                .into())
            }
        }
    }
}
