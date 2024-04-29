use std::sync::Arc;

use async_trait::async_trait;
use celestia_rpc::prelude::*;
use celestia_types::blob::{Blob as JsonBlob, SubmitOptions};
use celestia_types::consts::appconsts::{
    CONTINUATION_SPARSE_SHARE_CONTENT_SIZE, FIRST_SPARSE_SHARE_CONTENT_SIZE, SHARE_SIZE,
};
use celestia_types::nmt::Namespace;
use futures::stream::BoxStream;
use futures::StreamExt;
use jsonrpsee::http_client::{HeaderMap, HttpClient};
use sov_rollup_interface::da::{DaProof, RelevantBlobs, RelevantProofs};
use sov_rollup_interface::services::da::DaService;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument, trace};

use crate::types::{FilteredCelestiaBlock, NamespaceWithShares};
use crate::utils::BoxError;
use crate::verifier::address::CelestiaAddress;
use crate::verifier::proofs::{self};
use crate::verifier::{CelestiaSpec, CelestiaVerifier, RollupParams, PFB_NAMESPACE};
use crate::CelestiaHeader;

// Approximate value, just to make it work.
// https://github.com/celestiaorg/celestia-app/blob/c90e61d5a2d0c0bd0e123df4ab416f6f0d141b7f/pkg/appconsts/initial_consts.go#L16-L18
// By default it is 8, but upgraded to 10, to be on the safer side
const GAS_PER_BYTE: usize = 10;
// Fixed gas cost for blob calculation. Should be 65_000 in newer Celestia version.
const FIXED_COST: usize = 75_000;
// TODO: set dynamically https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/391
// Unit is uTIA.
// 1 uTIA = 10^-6 TIA (https://docs.celestia.org/learn/tia#tia-at-a-glance
const GAS_PRICE: usize = 1;

#[derive(Debug, Clone)]
pub struct CelestiaService {
    // https://github.com/celestiaorg/celestia-node/issues/3192
    client: Arc<Mutex<HttpClient>>,
    rollup_batch_namespace: Namespace,
    rollup_proof_namespace: Namespace,
}

impl CelestiaService {
    pub fn with_client(
        client: HttpClient,
        rollup_batch_namespace: Namespace,
        rollup_proof_namespace: Namespace,
    ) -> Self {
        Self {
            client: Arc::new(Mutex::new(client)),
            rollup_batch_namespace,
            rollup_proof_namespace,
        }
    }
}

/// Runtime configuration for the [`DaService`] implementation.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CelestiaConfig {
    /// The JWT used to authenticate with the Celestia RPC server
    pub celestia_rpc_auth_token: String,
    /// The address of the Celestia RPC server
    #[serde(default = "default_rpc_addr")]
    pub celestia_rpc_address: String,
    /// The maximum size of a Celestia RPC response, in bytes
    #[serde(default = "default_max_response_size")]
    pub max_celestia_response_body_size: u32,
    /// The timeout for a Celestia RPC request, in seconds
    #[serde(default = "default_request_timeout_seconds")]
    pub celestia_rpc_timeout_seconds: u64,
    /// Celestia address of connected node. Used as sequencer address in case of sequencer presented
    pub own_celestia_address: CelestiaAddress,
}

fn default_rpc_addr() -> String {
    "http://localhost:11111/".into()
}

fn default_max_response_size() -> u32 {
    1024 * 1024 * 100 // 100 MB
}

const fn default_request_timeout_seconds() -> u64 {
    60
}

impl CelestiaService {
    pub async fn new(config: CelestiaConfig, chain_params: RollupParams) -> Self {
        let client = {
            let mut headers = HeaderMap::new();
            headers.insert(
                "Authorization",
                format!("Bearer {}", config.celestia_rpc_auth_token)
                    .parse()
                    .unwrap(),
            );

            jsonrpsee::http_client::HttpClientBuilder::default()
                .set_headers(headers)
                .max_response_size(config.max_celestia_response_body_size)
                .max_request_size(config.max_celestia_response_body_size)
                .request_timeout(std::time::Duration::from_secs(
                    config.celestia_rpc_timeout_seconds,
                ))
                .build(&config.celestia_rpc_address)
        }
        .expect("Client initialization is valid");

        Self::with_client(
            client,
            chain_params.rollup_batch_namespace,
            chain_params.rollup_proof_namespace,
        )
    }
}

#[async_trait]
impl DaService for CelestiaService {
    type Spec = CelestiaSpec;

    type Verifier = CelestiaVerifier;

    type FilteredBlock = FilteredCelestiaBlock;
    type HeaderStream = BoxStream<'static, anyhow::Result<CelestiaHeader>>;
    type TransactionId = ();
    type Error = BoxError;

    #[instrument(skip(self), err)]
    async fn get_block_at(&self, height: u64) -> Result<Self::FilteredBlock, Self::Error> {
        let client = self.client.lock().await;

        // Fetch the header and relevant shares via RPC
        debug!(height, "Fetching header at height...");
        let header = client.header_get_by_height(height).await?;
        trace!(?header, height, "Got the header");

        // Fetch the rollup namespace shares, etx data and extended data square
        debug!("Fetching rollup data...");

        let etx_rows_future = client.share_get_shares_by_namespace(&header, PFB_NAMESPACE);
        let data_square_future = client.share_get_eds(&header);

        let rollup_batch_rows_future =
            client.share_get_shares_by_namespace(&header, self.rollup_batch_namespace);

        let rollup_proof_rows_future =
            client.share_get_shares_by_namespace(&header, self.rollup_proof_namespace);

        let (batch_rows, proof_rows, etx_rows, data_square) = tokio::try_join!(
            rollup_batch_rows_future,
            rollup_proof_rows_future,
            etx_rows_future,
            data_square_future
        )?;

        let rollup_batch_shares = NamespaceWithShares {
            namespace: self.rollup_batch_namespace,
            rows: batch_rows,
        };

        let rollup_proof_shares = NamespaceWithShares {
            namespace: self.rollup_proof_namespace,
            rows: proof_rows,
        };

        FilteredCelestiaBlock::new(
            rollup_batch_shares,
            rollup_proof_shares,
            header,
            etx_rows,
            data_square,
        )
    }

    async fn get_last_finalized_block_header(
        &self,
    ) -> Result<<Self::Spec as sov_rollup_interface::da::DaSpec>::BlockHeader, Self::Error> {
        // Tendermint has instant finality, so head block is the one that finalized
        // and network is always guaranteed to be secure,
        // it can work even if the node is still catching up
        self.get_head_block_header().await
    }

    async fn subscribe_finalized_header(&self) -> Result<Self::HeaderStream, Self::Error> {
        Ok(self
            .client
            .lock()
            .await
            .header_subscribe()
            .await?
            .map(|res| res.map(CelestiaHeader::from).map_err(Into::into))
            .boxed())
    }

    async fn get_head_block_header(
        &self,
    ) -> Result<<Self::Spec as sov_rollup_interface::da::DaSpec>::BlockHeader, Self::Error> {
        let header = self.client.lock().await.header_network_head().await?;
        Ok(CelestiaHeader::from(header))
    }

    fn extract_relevant_blobs(
        &self,
        block: &Self::FilteredBlock,
    ) -> RelevantBlobs<<Self::Spec as sov_rollup_interface::da::DaSpec>::BlobTransaction> {
        let proof_blobs = block.rollup_proof_data.get_blob_with_sender();
        let batch_blobs = block.rollup_batch_data.get_blob_with_sender();
        RelevantBlobs {
            proof_blobs,
            batch_blobs,
        }
    }

    async fn get_extraction_proof(
        &self,
        block: &Self::FilteredBlock,
        blobs: &RelevantBlobs<<Self::Spec as sov_rollup_interface::da::DaSpec>::BlobTransaction>,
    ) -> RelevantProofs<
        <Self::Spec as sov_rollup_interface::da::DaSpec>::InclusionMultiProof,
        <Self::Spec as sov_rollup_interface::da::DaSpec>::CompletenessProof,
    > {
        let batch = {
            let inclusion_proof = proofs::new_inclusion_proof(
                &block.header,
                &block.pfb_rows,
                &block.rollup_batch_data,
                &blobs.batch_blobs,
            );

            DaProof {
                inclusion_proof,
                completeness_proof: block.rollup_batch_data.rows.clone(),
            }
        };

        let proof = {
            // Note: The second call to new_inclusion_proof merklizes and parse the exectuable transactions namespace again.
            let inclusion_proof = proofs::new_inclusion_proof(
                &block.header,
                &block.pfb_rows,
                &block.rollup_proof_data,
                &blobs.proof_blobs,
            );

            DaProof {
                inclusion_proof,
                completeness_proof: block.rollup_proof_data.rows.clone(),
            }
        };

        RelevantProofs { proof, batch }
    }

    #[instrument(skip_all, err)]
    async fn send_transaction(&self, blob: &[u8]) -> Result<(), Self::Error> {
        let bytes = blob.len();
        debug!(bytes = bytes, "Sending raw data to Celestia");

        let gas_limit = get_gas_limit_for_bytes(bytes, GAS_PER_BYTE) as u64;
        // TODO: Correct fee calculation: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/382
        let fee = gas_limit.saturating_mul(GAS_PRICE as u64);

        let blob = JsonBlob::new(self.rollup_batch_namespace, blob.to_vec())?;
        info!(
            commitment = hex::encode(blob.commitment.0),
            gas_limit, fee, bytes, "Submitting a blob"
        );

        let height = self
            .client
            .lock()
            .await
            .blob_submit(
                &[blob],
                SubmitOptions {
                    fee: Some(fee),
                    gas_limit: Some(gas_limit),
                },
            )
            .await?;
        info!(height, "Blob has been submitted to Celestia");
        Ok(())
    }

    async fn send_aggregated_zk_proof(&self, aggregated_proof: &[u8]) -> Result<(), Self::Error> {
        let gas_limit = get_gas_limit_for_bytes(aggregated_proof.len(), GAS_PER_BYTE) as u64;
        let fee = gas_limit.saturating_mul(GAS_PRICE as u64);
        let blob = JsonBlob::new(self.rollup_proof_namespace, aggregated_proof.to_vec())?;

        let _height = self
            .client
            .lock()
            .await
            .blob_submit(
                &[blob],
                SubmitOptions {
                    fee: Some(fee),
                    gas_limit: Some(gas_limit),
                },
            )
            .await?;

        Ok(())
    }

    async fn get_aggregated_proofs_at(&self, height: u64) -> Result<Vec<Vec<u8>>, Self::Error> {
        let blobs = self
            .client
            .lock()
            .await
            .blob_get_all(height, &[self.rollup_proof_namespace])
            .await;

        match blobs {
            Ok(blobs) => Ok(blobs.into_iter().map(|blob| blob.data).collect()),
            // If 'Celestia' encounters a missing blob, it returns an error instead of an "empty" result.
            // Thus, we address this scenario here.
            // https://github.com/celestiaorg/celestia-node/issues/3192
            Err(e) => {
                info!("Get aggregated proof error (might happen if there is no blobs, that's expected): {}", e);
                Ok(Vec::default())
            }
        }
    }
}

// https://docs.celestia.org/developers/submit-data#fees-and-gas-limits
// Gas Limit is calculated as a fixed cost (FC) plus the sum of the product of the size of each blob (SSN(Bi))
// times the share size (SS) and the gas cost per byte blob (GCPBB) for each blob involved in the transaction.
// Gas Limit = FC + Σ(from i=1 to n) SSN(Bi) * SS * GCPBB
// where:
// FC = fixed cost
// SSN(Bi) = number of shares needed for the i-th blob
// SS = share size
// GCPBB = gas cost per byte
//
// Note, that often this function is called for calculating single blob gas limit, so we can simplify it to:
// Gas Limit = SSN(B) * SS * GCPBB + FC
// To yield optimal gas limit it needs further testing.
// For example, we are adding fixed cost for each blob, when node adds it to all blobs.
fn get_gas_limit_for_bytes(n: usize, gas_per_byte: usize) -> usize {
    debug_assert_ne!(CONTINUATION_SPARSE_SHARE_CONTENT_SIZE, 0);
    let continuation_shares_needed = n
        .saturating_sub(FIRST_SPARSE_SHARE_CONTENT_SIZE)
        .saturating_div(CONTINUATION_SPARSE_SHARE_CONTENT_SIZE);
    // 1 full share anyway + continuation shares
    let shares_needed = continuation_shares_needed.saturating_add(1);

    shares_needed
        .saturating_mul(SHARE_SIZE)
        .saturating_mul(gas_per_byte)
        .saturating_add(FIXED_COST)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::time::Duration;

    use celestia_types::nmt::Namespace;
    use celestia_types::Blob as JsonBlob;
    use serde_json::json;
    use sov_rollup_interface::da::{BlockHeaderTrait, DaVerifier, RelevantBlobs};
    use sov_rollup_interface::services::da::DaService;
    use wiremock::matchers::{bearer_token, body_json, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::{default_request_timeout_seconds, GAS_PER_BYTE};
    use crate::da_service::{get_gas_limit_for_bytes, CelestiaConfig, CelestiaService, GAS_PRICE};
    use crate::test_helper::files::*;
    use crate::types::FilteredCelestiaBlock;
    use crate::verifier::address::CelestiaAddress;
    use crate::verifier::{CelestiaVerifier, RollupParams};

    async fn setup_test_service(
        timeout_sec: Option<u64>,
    ) -> (MockServer, CelestiaConfig, CelestiaService, RollupParams) {
        setup_service(
            timeout_sec,
            CelestiaAddress::from_str("celestia1a68m2l85zn5xh0l07clk4rfvnezhywc53g8x7s").unwrap(),
            ROLLUP_BATCH_NAMESPACE,
            ROLLUP_PROOF_NAMESPACE,
        )
        .await
    }

    // Last return value is namespace
    async fn setup_service(
        timeout_sec: Option<u64>,
        celestia_address: CelestiaAddress,
        rollup_batch_namespace: Namespace,
        rollup_proof_namespace: Namespace,
    ) -> (MockServer, CelestiaConfig, CelestiaService, RollupParams) {
        // Start a background HTTP server on a random local port
        let mock_server = MockServer::start().await;

        let timeout_sec = timeout_sec.unwrap_or_else(default_request_timeout_seconds);
        let config = CelestiaConfig {
            celestia_rpc_auth_token: "RPC_TOKEN".to_string(),
            celestia_rpc_address: mock_server.uri(),
            max_celestia_response_body_size: 120_000,
            celestia_rpc_timeout_seconds: timeout_sec,
            own_celestia_address: celestia_address,
        };

        let params = RollupParams {
            rollup_batch_namespace,
            rollup_proof_namespace,
        };

        let da_service = CelestiaService::new(config.clone(), params.clone()).await;

        (mock_server, config, da_service, params)
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct BasicJsonRpcRequest {
        jsonrpc: String,
        id: u64,
        method: String,
        params: serde_json::Value,
    }

    #[tokio::test]
    async fn test_submit_blob_correct() -> anyhow::Result<()> {
        let (mock_server, config, da_service, rollup_params) = setup_test_service(None).await;

        let blob = [1, 2, 3, 4, 5, 11, 12, 13, 14, 15];
        let gas_limit = get_gas_limit_for_bytes(blob.len(), GAS_PER_BYTE);

        // TODO: Fee is hardcoded for now https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/382
        let expected_body = json!({
            "id": 0,
            "jsonrpc": "2.0",
            "method": "blob.Submit",
            "params": [
                [JsonBlob::new(rollup_params.rollup_batch_namespace, blob.to_vec()).unwrap()],
                {
                    "GasLimit": gas_limit,
                    "Fee": gas_limit * GAS_PRICE,
                },
            ]
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .and(bearer_token(config.celestia_rpc_auth_token))
            .and(body_json(&expected_body))
            .respond_with(|req: &Request| {
                let request: BasicJsonRpcRequest = serde_json::from_slice(&req.body).unwrap();
                let response_json = json!({
                    "jsonrpc": "2.0",
                    "id": request.id,
                    "result": 14, // just some block-height
                });

                ResponseTemplate::new(200)
                    .append_header("Content-Type", "application/json")
                    .set_body_json(response_json)
            })
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        da_service.send_transaction(&blob).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_submit_blob_application_level_error() -> anyhow::Result<()> {
        // Our calculation of gas is off and gas limit exceeded, for example
        let (mock_server, _config, da_service, _namespace) = setup_test_service(None).await;

        let blob: Vec<u8> = vec![1, 2, 3, 4, 5, 11, 12, 13, 14, 15];

        // Do not check API token or expected body here.
        // Only interested in behaviour on response
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(|req: &Request| {
                let request: BasicJsonRpcRequest = serde_json::from_slice(&req.body).unwrap();
                let response_json = json!({
                    "jsonrpc": "2.0",
                    "id": request.id,
                    "error": {
                        "code": 1,
                        "message": ": out of gas"
                    }
                });
                ResponseTemplate::new(200)
                    .append_header("Content-Type", "application/json")
                    .set_body_json(response_json)
            })
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        let error = da_service
            .send_transaction(&blob)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("out of gas"));
        Ok(())
    }

    #[tokio::test]
    async fn test_submit_blob_internal_server_error() -> anyhow::Result<()> {
        let (mock_server, _config, da_service, _namespace) = setup_test_service(None).await;

        let error_response = ResponseTemplate::new(500).set_body_bytes("Internal Error".as_bytes());

        let blob: Vec<u8> = vec![1, 2, 3, 4, 5, 11, 12, 13, 14, 15];

        // Do not check API token or expected body here.
        // Only interested in behaviour on response
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(error_response)
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        let error = da_service
            .send_transaction(&blob)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains(
            "Networking or low-level protocol error: Server returned an error status code: 500"
        ));
        Ok(())
    }

    #[tokio::test]
    // This test is slow now, but it can be fixed when
    // https://github.com/Sovereign-Labs/sovereign-sdk/issues/478 is implemented
    // Slower request timeout can be set
    async fn test_submit_blob_response_timeout() -> anyhow::Result<()> {
        let timeout = 1;
        let (mock_server, _config, da_service, _namespace) =
            setup_test_service(Some(timeout)).await;

        let response_json = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "result": {
                "data": "122A0A282F365",
                "events": ["some event"],
                "gas_used": 70522,
                "gas_wanted": 133540,
                "height": 26,
                "logs":  [
                   "some log"
                ],
                "raw_log": "some raw logs",
                "txhash": "C9FEFD6D35FCC73F9E7D5C74E1D33F0B7666936876F2AD75E5D0FB2944BFADF2"
            }
        });

        let error_response = ResponseTemplate::new(200)
            .append_header("Content-Type", "application/json")
            .set_delay(Duration::from_secs(timeout) + Duration::from_millis(100))
            .set_body_json(response_json);

        let blob: Vec<u8> = vec![1, 2, 3, 4, 5, 11, 12, 13, 14, 15];

        // Do not check API token or expected body here.
        // Only interested in behaviour on response
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(error_response)
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        let error = da_service
            .send_transaction(&blob)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("Request timeout"));
        Ok(())
    }

    #[tokio::test]
    async fn verification_succeeds_for_correct_blocks() {
        let blocks = [
            with_rollup_batch_data::filtered_block(),
            without_rollup_batch_data::filtered_block(),
        ];

        for block in blocks {
            let (_, _, da_service, rollup_params) = setup_test_service(None).await;

            let relevant_blobs = da_service.extract_relevant_blobs(&block);
            let relevant_proofs = da_service
                .get_extraction_proof(&block, &relevant_blobs)
                .await;

            let verifier = CelestiaVerifier::new(rollup_params);

            let validity_cond = verifier
                .verify_relevant_tx_list(&block.header, &relevant_blobs, relevant_proofs)
                .unwrap();

            assert_eq!(validity_cond.prev_hash, *block.header.prev_hash().inner());
            assert_eq!(validity_cond.block_hash, *block.header.hash().inner());
        }
    }

    #[tokio::test]
    async fn verification_fails_if_tx_missing() {
        let block = with_rollup_batch_data::filtered_block();
        let (_, _, da_service, rollup_params) = setup_test_service(None).await;

        let relevant_blobs = da_service.extract_relevant_blobs(&block);
        let relevant_proofs = da_service
            .get_extraction_proof(&block, &relevant_blobs)
            .await;

        let verifier = CelestiaVerifier::new(rollup_params);

        let relevant_blobs = RelevantBlobs {
            proof_blobs: Default::default(),
            batch_blobs: Default::default(),
        };
        // give verifier empty txs list
        let error = verifier
            .verify_relevant_tx_list(&block.header, &relevant_blobs, relevant_proofs)
            .unwrap_err();

        assert!(error.to_string().contains("Transaction missing"));
    }

    #[tokio::test]
    async fn verification_fails_if_not_all_etxs_are_proven() {
        let block = with_rollup_batch_data::filtered_block();
        let (_, _, da_service, rollup_params) = setup_test_service(None).await;

        let relevant_blobs = da_service.extract_relevant_blobs(&block);

        let mut relevant_proofs = da_service
            .get_extraction_proof(&block, &relevant_blobs)
            .await;
        // drop the proof for last etx
        relevant_proofs.batch.inclusion_proof.pop();

        let verifier = CelestiaVerifier::new(rollup_params);

        let error = verifier
            .verify_relevant_tx_list(&block.header, &relevant_blobs, relevant_proofs)
            .unwrap_err();

        assert!(error.to_string().contains("not all blobs proven"));
    }

    #[tokio::test]
    async fn test_blobs_from_padded_namespace() {
        let block: FilteredCelestiaBlock = with_namespace_padding::filtered_block();
        let (_, _, da_service, _) = setup_service(
            None,
            CelestiaAddress::from_str("celestia1g2hwtcldcwjnw0cy9ngs9hsewpduq4zuehqlqh").unwrap(),
            with_namespace_padding::ROLLUP_BATCH_NAMESPACE,
            with_namespace_padding::ROLLUP_PROOF_NAMESPACE,
        )
        .await;
        let relevant_blobs = da_service.extract_relevant_blobs(&block);
        assert_eq!(relevant_blobs.batch_blobs.len(), 1);
        assert_eq!(relevant_blobs.proof_blobs.len(), 0);
    }

    // TODO: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/430
    #[tokio::test]
    #[ignore = "TODO: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/430"]
    async fn _verification_for_padded_namespace() {
        let block: FilteredCelestiaBlock = with_namespace_padding::filtered_block();
        let (_, _, da_service, rollup_params) = setup_service(
            None,
            CelestiaAddress::from_str("celestia1g2hwtcldcwjnw0cy9ngs9hsewpduq4zuehqlqh").unwrap(),
            with_namespace_padding::ROLLUP_BATCH_NAMESPACE,
            with_namespace_padding::ROLLUP_PROOF_NAMESPACE,
        )
        .await;

        let relevant_blobs = da_service.extract_relevant_blobs(&block);
        let relevant_proofs = da_service
            .get_extraction_proof(&block, &relevant_blobs)
            .await;

        let verifier = CelestiaVerifier::new(rollup_params);

        let _validity_cond = verifier
            .verify_relevant_tx_list(&block.header, &relevant_blobs, relevant_proofs)
            .unwrap();
    }

    #[tokio::test]
    async fn verification_fails_if_there_is_less_blobs_than_proofs() {
        let block = with_rollup_batch_data::filtered_block();
        let (_, _, da_service, rollup_params) = setup_test_service(None).await;

        let relevant_blobs = da_service.extract_relevant_blobs(&block);
        let mut relevant_proofs = da_service
            .get_extraction_proof(&block, &relevant_blobs)
            .await;

        // push one extra etx proof
        relevant_proofs
            .batch
            .inclusion_proof
            .push(relevant_proofs.batch.inclusion_proof[0].clone());

        let verifier = CelestiaVerifier::new(rollup_params);

        let error = verifier
            .verify_relevant_tx_list(&block.header, &relevant_blobs, relevant_proofs)
            .unwrap_err();

        assert!(error.to_string().contains("more proofs than blobs"));
    }

    #[tokio::test]
    #[should_panic]
    async fn verification_fails_for_incorrect_namespace() {
        let block = with_rollup_proof_data::filtered_block();
        let (_, _, da_service, _) = setup_test_service(None).await;

        let relevant_blobs = da_service.extract_relevant_blobs(&block);
        let relevant_proofs = da_service
            .get_extraction_proof(&block, &relevant_blobs)
            .await;

        // create a verifier with a different namespace than the da_service
        let verifier = CelestiaVerifier::new(RollupParams {
            rollup_proof_namespace: Namespace::new_v0(b"abc").unwrap(),
            rollup_batch_namespace: Namespace::new_v0(b"xyz").unwrap(),
        });

        let _panics =
            verifier.verify_relevant_tx_list(&block.header, &relevant_blobs, relevant_proofs);
    }

    #[tokio::test]
    async fn test_submit_proof() -> anyhow::Result<()> {
        let (mock_server, config, da_service, rollup_params) = setup_test_service(None).await;

        let zk_proof: Vec<u8> = vec![1, 2, 3, 4, 5, 11, 12, 13, 14, 15];
        let gas_limit = get_gas_limit_for_bytes(zk_proof.len(), GAS_PER_BYTE);

        let expected_body = json!({
            "id": 0,
            "jsonrpc": "2.0",
            "method": "blob.Submit",
            "params": [
                [JsonBlob::new(rollup_params.rollup_proof_namespace, zk_proof.to_vec()).unwrap()],
                {
                    "GasLimit": gas_limit,
                    "Fee": gas_limit * GAS_PRICE,
                },
            ]
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .and(bearer_token(config.celestia_rpc_auth_token))
            .and(body_json(&expected_body))
            .respond_with(|req: &Request| {
                let request: BasicJsonRpcRequest = serde_json::from_slice(&req.body).unwrap();
                let response_json = json!({
                    "jsonrpc": "2.0",
                    "id": request.id,
                    "result": 14, // just some block-height
                });

                ResponseTemplate::new(200)
                    .append_header("Content-Type", "application/json")
                    .set_body_json(response_json)
            })
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        da_service.send_aggregated_zk_proof(&zk_proof).await?;

        Ok(())
    }

    #[test]
    fn test_gas_limit_for_bytes() {
        // 0 bytes
        // 1 byte
        // 100 KB
        // 500 KB
        // 1 MB
        // 5 MB
        // 10 MB
        // 100 MB
        // 1 GB
        let cases = vec![
            (0, 80120),
            (1, 80120),
            (102400, 1160440),
            (512000, 5512440),
            (1048576, 11211000),
            (5242880, 55765240),
            (10485760, 111455480),
            (104857600, 1113910520),
            (1073741824, 11405796600),
            (usize::MAX, usize::MAX),
        ];

        for (blob_size, expected_gas_limit) in cases {
            let gas_limit = get_gas_limit_for_bytes(blob_size, GAS_PER_BYTE);
            // To update test uncomment this and comment assert.
            // Then put it back after data is updated. Don't forget to not replace last use case
            // println!("({}, {}),", blob_size, gas_limit);
            assert_eq!(gas_limit, expected_gas_limit);
        }
    }

    #[test]
    fn sanity_check_fee_with_current_testnet() {
        // https://mocha.celenium.io/tx/7b8dd68a7a8542714dfbb1b655a381d71ce013b0fc406acc3a56b61a116e7253
        // {
        //   "id": 3068781,
        //   "gas_wanted": 904440,
        //   "gas_used": 395479,
        //   "hash": "7b8dd68a7a8542714dfbb1b655a381d71ce013b0fc406acc3a56b61a116e7253",
        //   "fee": "904440",
        //   "time": "2024-04-01T11:38:39.407177Z",
        // TX:
        // {
        //   "id": 3077971,
        //   "type": "MsgPayForBlobs",
        //   "data": {
        //     "BlobSizes": [
        //       38617
        //     ],
        // }

        let blob_size = 38617;
        let gas_wanted = 904440;
        let gas_used = 395479;
        let gas_used_upper_bound = (gas_wanted as f64 * 1.4) as usize;

        let gas_limit = get_gas_limit_for_bytes(blob_size, GAS_PER_BYTE);

        assert!(gas_limit >= gas_used);
        assert!(gas_limit <= gas_used_upper_bound);
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10000))]
        #[test]
        fn get_gas_limit_for_bytes_does_not_panic_test(
            blob_size in any::<usize>(),
            gas_per_bytes in any::<usize>(),
        ) {
            let _ = get_gas_limit_for_bytes(blob_size, gas_per_bytes);
        }
    }
}
