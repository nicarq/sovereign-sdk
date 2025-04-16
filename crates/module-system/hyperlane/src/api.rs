use axum::routing::get;
use serde::Serialize;
use sov_modules_api::prelude::utoipa::openapi::OpenApi;
use sov_modules_api::prelude::{axum, UnwrapInfallible};
use sov_modules_api::rest::utils::{errors, ApiResult, Path};
use sov_modules_api::rest::{ApiState, HasCustomRestApi};
use sov_modules_api::{ApiStateAccessor, HexHash, Spec};

use crate::{EthAddress, Ism, Mailbox, Recipient};

/// A configuration of an [`Ism::MessageIdMultisig`].
#[derive(Serialize)]
pub struct ValidatorsAndThreshold {
    /// The addresses of the validators
    validators: Vec<EthAddress>,
    /// The number of signatures required to accept a message
    threshold: u32,
}

impl<S: Spec, R: Recipient<S>> HasCustomRestApi for Mailbox<S, R> {
    type Spec = S;

    fn custom_rest_api(&self, state: ApiState<S>) -> axum::Router<()> {
        axum::Router::new()
            .route("/nonce", get(Self::get_nonce))
            .route("/recipient-ism/:address", get(Self::get_recipient_ism))
            .route(
                "/recipient-ism/:address/validators_and_threshold",
                get(Self::get_recipient_ism_validators_and_threshold),
            )
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

    async fn get_recipient_ism_validators_and_threshold(
        state: ApiState<S, Self>,
        Path(address): Path<HexHash>,
        mut accessor: ApiStateAccessor<S>,
    ) -> ApiResult<ValidatorsAndThreshold> {
        let ism = state
            .recipients
            .ism(&address, &mut accessor)
            .map_err(|_| errors::not_found_404("Mailbox", address))?
            .ok_or_else(|| errors::not_found_404("Mailbox", address))?;

        let Ism::MessageIdMultisig {
            validators,
            threshold,
        } = ism
        else {
            return Err(errors::bad_request_400(
                "Failed getting validators and threshold",
                "Ism is not of type MessageIdMultisig",
            ));
        };

        Ok(ValidatorsAndThreshold {
            validators: validators.into(),
            threshold,
        }
        .into())
    }
}
