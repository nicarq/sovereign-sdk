use serde::Deserialize;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::rest::utils::ResponseObject;

use crate::test_helpers::{get_appropriate_rollup_prover_config, TestRollup};

#[derive(Debug, Deserialize)]
struct ValueResponse {
    value: u64,
}

#[tokio::test(flavor = "multi_thread")]
async fn trailing_slashes_handled() -> anyhow::Result<()> {
    let test_rollup = TestRollup::create_test_rollup(
        get_appropriate_rollup_prover_config(),
        BlockProducingConfig::OnSubmit,
        0,
    )
    .await?;

    let response = test_rollup
        .client
        .query_rest_endpoint::<ResponseObject<ValueResponse>>(
            "/modules/attester-incentives/state/minimum-challenger-bond",
        )
        .await?;

    let bond = response.data.unwrap().value;

    let response = test_rollup
        .client
        .query_rest_endpoint::<ResponseObject<ValueResponse>>(
            "/modules/attester-incentives/state/minimum-challenger-bond/",
        )
        .await?;

    assert_eq!(Some(bond), response.data.map(|d| d.value));

    let swagger_ui_url_1 = test_rollup.client.http_get("/swagger-ui").await?;
    let swagger_ui_url_2 = test_rollup.client.http_get("/swagger-ui/").await?;

    assert_eq!(swagger_ui_url_1, swagger_ui_url_2);

    Ok(())
}
