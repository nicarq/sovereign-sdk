pub use gas_price::gas_oracle::GasPriceOracleConfig;
#[cfg(feature = "local")]
pub use sov_eth_dev_signer::DevSigner;

mod batch_builder;
mod gas_price;
#[cfg(feature = "local")]
mod signer;

use std::sync::{Arc, Mutex};

use borsh::ser::BorshSerialize;
use demo_stf::runtime::Runtime;
use jsonrpsee::types::ErrorObjectOwned;
use jsonrpsee::RpcModule;
use reth_primitives::{Bytes, TransactionSignedNoHash as RethTransactionSignedNoHash, B256, U256};
use sov_evm::{CallMessage, Evm, RlpEvmTransaction};
use sov_modules_api::utils::to_jsonrpsee_error_object;
use sov_modules_api::{CryptoSpec, EncodeCall, PrivateKey, WorkingSet};
use sov_rollup_interface::services::da::DaService;
use tokio::sync::watch;

use crate::batch_builder::EthBatchBuilder;
use crate::gas_price::gas_oracle::GasPriceOracle;

const ETH_RPC_ERROR: &str = "ETH_RPC_ERROR";

#[derive(Clone)]
pub struct EthRpcConfig<S: sov_modules_api::Spec> {
    pub min_blob_size: Option<usize>,
    pub sov_tx_signer_priv_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    pub gas_price_oracle_config: GasPriceOracleConfig,
    #[cfg(feature = "local")]
    pub eth_signer: DevSigner,
}

pub fn get_ethereum_rpc<S: sov_modules_api::Spec, Da: DaService>(
    da_service: Da,
    eth_rpc_config: EthRpcConfig<S>,
    storage: watch::Receiver<S::Storage>,
) -> RpcModule<Ethereum<S, Da>> {
    // Unpack config
    let EthRpcConfig {
        min_blob_size,
        sov_tx_signer_priv_key,
        #[cfg(feature = "local")]
        eth_signer,
        gas_price_oracle_config,
    } = eth_rpc_config;

    // Fetch nonce from storage
    let accounts = sov_accounts::Accounts::<S>::default();
    let sov_tx_signer_account = accounts
        .get_account(
            sov_tx_signer_priv_key.pub_key(),
            &mut WorkingSet::<S>::new(storage.borrow().clone()),
        )
        .unwrap();
    let sov_tx_signer_nonce: u64 = match sov_tx_signer_account {
        sov_accounts::Response::AccountExists { nonce, .. } => nonce,
        sov_accounts::Response::AccountEmpty { .. } => 0,
    };

    let mut rpc = RpcModule::new(Ethereum::new(
        da_service,
        Arc::new(Mutex::new(EthBatchBuilder::new(
            sov_tx_signer_priv_key,
            sov_tx_signer_nonce,
            min_blob_size,
        ))),
        gas_price_oracle_config,
        #[cfg(feature = "local")]
        eth_signer,
        storage,
    ));

    register_rpc_methods(&mut rpc).expect("Failed to register sequencer RPC methods");
    rpc
}

pub struct Ethereum<S: sov_modules_api::Spec, Da: DaService> {
    da_service: Da,
    batch_builder: Arc<Mutex<EthBatchBuilder<S>>>,
    gas_price_oracle: GasPriceOracle<S, Da::Spec>,
    #[cfg(feature = "local")]
    eth_signer: DevSigner,
    storage: watch::Receiver<S::Storage>,
}

impl<S: sov_modules_api::Spec, Da: DaService> Ethereum<S, Da> {
    fn new(
        da_service: Da,
        batch_builder: Arc<Mutex<EthBatchBuilder<S>>>,
        gas_price_oracle_config: GasPriceOracleConfig,
        #[cfg(feature = "local")] eth_signer: DevSigner,
        storage: watch::Receiver<S::Storage>,
    ) -> Self {
        let evm = Evm::<S, Da::Spec>::default();
        let gas_price_oracle = GasPriceOracle::new(evm, gas_price_oracle_config);
        Self {
            da_service,
            batch_builder,
            gas_price_oracle,
            #[cfg(feature = "local")]
            eth_signer,
            storage,
        }
    }
}

impl<S: sov_modules_api::Spec, Da: DaService> Ethereum<S, Da> {
    fn make_raw_tx(
        &self,
        raw_tx: RlpEvmTransaction,
    ) -> Result<(B256, Vec<u8>), jsonrpsee::core::Error> {
        let signed_transaction: RethTransactionSignedNoHash = raw_tx.clone().try_into()?;

        let tx_hash = signed_transaction.hash();

        let tx = CallMessage { tx: raw_tx };
        let message = <Runtime<S, Da::Spec> as EncodeCall<Evm<S, Da::Spec>>>::encode_call(tx);

        Ok((tx_hash, message))
    }

    async fn build_and_submit_batch(
        &self,
        messages: Vec<Vec<u8>>,
        min_blob_size: Option<usize>,
    ) -> Result<(), jsonrpsee::core::Error> {
        tracing::info!(
            messages = messages.len(),
            min_blob_size = min_blob_size,
            "Build and submit ETH batch request has been received",
        );
        let batch = self.build_batch(messages, min_blob_size)?;
        tracing::debug!(transactions_count = batch.len(), "Batch have been built");

        self.submit_batch(batch)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, ETH_RPC_ERROR))?;

        Ok(())
    }

    async fn submit_batch(&self, batch: Vec<Vec<u8>>) -> Result<(), jsonrpsee::core::Error> {
        if batch.is_empty() {
            tracing::error!("Attempt to submit empty batch");
            return Err(jsonrpsee::core::Error::Custom(
                "Attempt to submit empty batch".to_string(),
            ));
        }
        let blob = batch
            .try_to_vec()
            .map_err(|e| to_jsonrpsee_error_object(e, ETH_RPC_ERROR))?;

        self.da_service
            .send_transaction(&blob)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, ETH_RPC_ERROR))?;
        tracing::debug!("ETH Batch has been submitted");
        Ok(())
    }

    fn build_batch(
        &self,
        messages: Vec<Vec<u8>>,
        min_blob_size: Option<usize>,
    ) -> Result<Vec<Vec<u8>>, jsonrpsee::core::Error> {
        let batch = self
            .batch_builder
            .lock()
            .unwrap()
            .add_messages_and_get_next_blob(min_blob_size, messages);

        Ok(batch)
    }

    fn add_messages(&self, messages: Vec<Vec<u8>>) {
        self.batch_builder.lock().unwrap().add_messages(messages);
    }
}

fn register_rpc_methods<S: sov_modules_api::Spec, Da: DaService>(
    rpc: &mut RpcModule<Ethereum<S, Da>>,
) -> Result<(), jsonrpsee::core::Error> {
    rpc.register_async_method("eth_gasPrice", |_, ethereum| async move {
        let price = {
            let mut working_set = WorkingSet::<S>::new(ethereum.storage.borrow().clone());

            let suggested_tip = ethereum
                .gas_price_oracle
                .suggest_tip_cap(&mut working_set)
                .await
                .unwrap();

            let evm = Evm::<S, Da::Spec>::default();
            let base_fee = evm
                .get_block_by_number(None, None, &mut working_set)
                .unwrap()
                .unwrap()
                .header
                .base_fee_per_gas
                .unwrap_or_default();

            suggested_tip + base_fee
        };

        Ok::<U256, ErrorObjectOwned>(price)
    })?;

    rpc.register_async_method("eth_publishBatch", |params, ethereum| async move {
        let mut params_iter = params.sequence();

        let mut txs = Vec::default();
        while let Some(tx) = params_iter.optional_next::<Vec<u8>>()? {
            txs.push(tx);
        }

        ethereum
            .build_and_submit_batch(txs, Some(1))
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, ETH_RPC_ERROR))?;

        Ok::<String, ErrorObjectOwned>("Submitted transaction".to_string())
    })?;

    rpc.register_async_method(
        "eth_sendRawTransaction",
        |parameters, ethereum| async move {
            let data: Bytes = parameters.one().unwrap();

            let raw_evm_tx = RlpEvmTransaction { rlp: data.to_vec() };

            let (tx_hash, raw_message) = ethereum
                .make_raw_tx(raw_evm_tx)
                .map_err(|e| to_jsonrpsee_error_object(e, ETH_RPC_ERROR))?;

            ethereum.add_messages(vec![raw_message]);

            Ok::<_, ErrorObjectOwned>(tx_hash)
        },
    )?;

    #[cfg(feature = "local")]
    signer::register_signer_rpc_methods(rpc)?;

    Ok(())
}
