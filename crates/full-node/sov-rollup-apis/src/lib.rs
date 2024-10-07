//! This crate contains API specification to interact with the gas module.

#![deny(missing_docs)]
use std::sync::OnceLock;

use axum::extract::State;
use axum::routing::{get, post};
use axum::Json;
use sov_modules_api::rest::StorageReceiver;
use sov_modules_api::transaction::TxDetails;
use sov_modules_api::{Gas, Spec, TxEffect, TxReceiptContents};
use sov_rest_utils::{errors, preconfigured_router_layers, ApiResult, ResponseObject};

/// Provides the `dedup` endpoint functionality.
pub mod dedup;
mod default_provider;

pub use default_provider::DefaultRollupStateProvider;

/// Use [`RollupTxRouter::axum_router`] to instantiate an [`axum::Router`] for
/// a specific [`RollupStateProvider`].
#[derive(Clone)]
pub struct RollupTxRouter<T: RollupStateProvider>(StorageReceiver<T::Spec>);

/// The object returned by the `/base-fee-per-gas/latest` endpoint.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound = "S: Spec")]
pub struct GasPriceContainer<S: Spec> {
    base_fee_per_gas: <S::Gas as Gas>::Price,
}

const RAW_YAML_SPEC: &str = include_str!("../openapi-v3.yaml");

/// Returns parsed [`openapiv3::OpenAPI`] for Ledger JSON API.
/// Performs clone of the whole spec, so might be slow.
pub fn open_api_v3_spec() -> openapiv3::OpenAPI {
    static OPENAPI_SPEC_V3: OnceLock<openapiv3::OpenAPI> = OnceLock::new();
    OPENAPI_SPEC_V3
        .get_or_init(|| serde_yaml::from_str(RAW_YAML_SPEC).unwrap())
        .clone()
}

/// A partial transaction type that can be used to easily simulate the execution of a transaction.
/// This type is `partial` in the sense that it does not contain the full transaction details.
#[derive(Debug, serde::Deserialize)]
#[serde(bound = "S: Spec")]
pub struct PartialTransaction<S: Spec> {
    /// The rollup address of the transaction sender.
    pub sender: S::Address,
    /// Call message encoded by a given runtime.
    pub encoded_call_message: Vec<u8>,
    /// The details of the transaction.
    pub details: TxDetails<S>,
}

/// A [`RollupStateProvider`] provides a way to query the state for information about gas.
pub trait RollupStateProvider: Clone + Send + Sync {
    /// The error type for fallible methods on this trait.
    type Error: ToString + Send + Sync + 'static;

    /// The spec associated with the rollup state provider.
    type Spec: sov_modules_api::Spec;

    /// The type of receipt that the rollup state provider returns.
    type Receipt: TxReceiptContents;

    /// Get the latest base fee per gas in the storage.
    fn get_latest_base_fee_per_gas(
        storage: &StorageReceiver<Self::Spec>,
    ) -> Result<<<Self::Spec as Spec>::Gas as Gas>::Price, Self::Error>;

    /// Simulates the execution of a transaction.
    fn simulate_execution(
        storage: &StorageReceiver<Self::Spec>,
        transaction: PartialTransaction<Self::Spec>,
    ) -> Result<TxEffect<Self::Receipt>, Self::Error>;
}

impl<T> RollupTxRouter<T>
where
    T: RollupStateProvider + Clone + Send + Sync + 'static,
{
    /// Returns an [`axum::Router`] that exposes simulation data.
    pub fn axum_router(storage: StorageReceiver<T::Spec>) -> axum::Router<()> {
        preconfigured_router_layers(
            axum::Router::new()
                .route(
                    "/rollup/base-fee-per-gas/latest",
                    get(Self::get_latest_base_fee_per_gas),
                )
                .route("/rollup/simulate-execution", post(Self::simulate_execution))
                .with_state(RollupTxRouter(storage)),
        )
    }

    /// Get the latest base fee per gas in the storage.
    async fn get_latest_base_fee_per_gas(
        State(RollupTxRouter(state_recv)): State<Self>,
    ) -> ApiResult<GasPriceContainer<T::Spec>> {
        match T::get_latest_base_fee_per_gas(&state_recv) {
            Ok(base_fee_per_gas) => {
                Ok(ResponseObject::from(GasPriceContainer { base_fee_per_gas }))
            }
            Err(err) => Err(errors::database_error_response_500(err)),
        }
    }

    /// Simulates the execution of a transaction.
    async fn simulate_execution(
        State(RollupTxRouter(state_recv)): State<Self>,
        Json(transaction): Json<PartialTransaction<T::Spec>>,
    ) -> ApiResult<TxEffect<T::Receipt>> {
        match T::simulate_execution(&state_recv, transaction) {
            Ok(tx_effect) => Ok(ResponseObject::from(tx_effect)),
            Err(err) => Err(errors::database_error_response_500(err)),
        }
    }
}
