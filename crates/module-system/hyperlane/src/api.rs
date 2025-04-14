use axum::routing::get;
use sov_modules_api::prelude::utoipa::openapi::OpenApi;
use sov_modules_api::prelude::{axum, UnwrapInfallible};
use sov_modules_api::rest::utils::{errors, ApiResult, Path};
use sov_modules_api::rest::{ApiState, HasCustomRestApi};
use sov_modules_api::{ApiStateAccessor, HexHash, Spec};

use crate::{Mailbox, Recipient};

impl<S: Spec, R: Recipient<S>> HasCustomRestApi for Mailbox<S, R> {
    type Spec = S;

    fn custom_rest_api(&self, state: ApiState<S>) -> axum::Router<()> {
        axum::Router::new()
            .route("/nonce", get(Self::get_nonce))
            .route("/recipient-ism/:address", get(Self::get_recipient_ism))
            .with_state(state.with(self.clone()))
    }

    fn custom_openapi_spec(&self) -> Option<OpenApi> {
        let mut open_api: OpenApi =
            serde_yaml::from_str(include_str!("openapi-v3.yaml")).expect("Invalid OpenAPI spec");
        // Because https://github.com/juhaku/utoipa/issues/972
        for path_item in open_api.paths.paths.values_mut() {
            path_item.extensions = None;
        }
        Some(open_api)
    }
}

impl<S: Spec, R: Recipient<S>> Mailbox<S, R> {
    async fn get_nonce(
        state: ApiState<S, Self>,
        mut accessor: ApiStateAccessor<S>,
    ) -> ApiResult<u32> {
        let nonce = state
            .dispatch_state
            .get(&mut accessor)
            .unwrap_infallible()
            .map(|dispatch_state| dispatch_state.nonce)
            .unwrap_or_default();
        Ok(nonce.into())
    }

    async fn get_recipient_ism(
        state: ApiState<S, Self>,
        Path(address): Path<HexHash>,
        mut accessor: ApiStateAccessor<S>,
    ) -> ApiResult<u8> {
        let ism = state
            .recipients
            .ism(&address, &mut accessor)
            .map_err(|_| errors::not_found_404("Mailbox", address))?
            .ok_or_else(|| errors::not_found_404("Mailbox", address))?;
        Ok((ism.ism_kind() as u8).into())
    }
}
