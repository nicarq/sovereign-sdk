use std::str::FromStr;

use anyhow::Context as _;
use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_ethereum::{EthRpcConfig, GasPriceOracleConfig};
use sov_modules_api::{CryptoSpec, Spec};
use sov_rollup_interface::services::da::DaService;
use tokio::sync::watch;

const TX_SIGNER_PRIV_KEY_PATH: &str = "../test-data/keys/tx_signer_private_key.json";

/// Ethereum RPC wraps EVM transaction in a rollup transaction.
/// This function reads the private key of the rollup transaction signer.
fn read_sov_tx_signer_priv_key<S: Spec>(
) -> Result<<<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey, anyhow::Error> {
    let tx_signer_key_path = std::env::var("SOV_TX_SIGNER_PRIV_KEY_PATH")
        .unwrap_or_else(|_| TX_SIGNER_PRIV_KEY_PATH.to_string());

    let data = std::fs::read_to_string(&tx_signer_key_path).with_context(|| {
        format!(
            "Unable to read sov ethereum tx signer key file from: {}",
            tx_signer_key_path
        )
    })?;

    let key_and_address: PrivateKeyAndAddress<S> =
        serde_json::from_str(&data).unwrap_or_else(|e| {
            panic!(
                "Unable to convert data {} to PrivateKeyAndAddress: {:?}",
                &data, e
            )
        });

    Ok(key_and_address.private_key)
}

// register ethereum methods.
pub(crate) fn register_ethereum<S: Spec, Da: DaService>(
    da_service: Da,
    storage: watch::Receiver<<S as Spec>::Storage>,
    methods: &mut jsonrpsee::RpcModule<()>,
) -> Result<(), anyhow::Error> {
    let eth_rpc_config = {
        let eth_signer = eth_dev_signer();
        EthRpcConfig::<S> {
            min_blob_size: Some(1),
            sov_tx_signer_priv_key: read_sov_tx_signer_priv_key::<S>()?,
            eth_signer,
            gas_price_oracle_config: GasPriceOracleConfig::default(),
        }
    };

    let ethereum_rpc = sov_ethereum::get_ethereum_rpc::<S, Da>(da_service, eth_rpc_config, storage);
    methods
        .merge(ethereum_rpc)
        .context("Failed to merge Ethereum RPC modules")
}

// TODO: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/387
fn eth_dev_signer() -> sov_ethereum::DevSigner {
    sov_ethereum::DevSigner::new(vec![secp256k1::SecretKey::from_str(
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    )
    .unwrap()])
}
