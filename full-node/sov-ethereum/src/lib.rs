#[cfg(feature = "experimental")]
mod batch_builder;
#[cfg(feature = "experimental")]
mod gas_price;

#[cfg(feature = "experimental")]
pub use experimental::{get_ethereum_rpc, Ethereum};
#[cfg(feature = "experimental")]
pub use gas_price::gas_oracle::GasPriceOracleConfig;
#[cfg(feature = "experimental")]
pub use sov_evm::DevSigner;

#[cfg(feature = "experimental")]
pub mod experimental {
    use std::sync::{Arc, Mutex};

    use borsh::ser::BorshSerialize;
    use demo_stf::runtime::Runtime;
    use jsonrpsee::types::ErrorObjectOwned;
    use jsonrpsee::RpcModule;
    use reth_primitives::{
        Address, Bytes, TransactionSignedNoHash as RethTransactionSignedNoHash, B256, U256, U64,
    };
    use reth_rpc_types::transaction::{
        EIP1559TransactionRequest, EIP2930TransactionRequest, EIP4844TransactionRequest,
        LegacyTransactionRequest,
    };
    use reth_rpc_types::{
        TransactionKind as RpcTransactionKind, TransactionRequest, TypedTransactionRequest,
    };
    use sov_evm::{CallMessage, EthApiError, Evm, RlpEvmTransaction};
    use sov_modules_api::utils::to_jsonrpsee_error_object;
    use sov_modules_api::{CryptoSpec, EncodeCall, PrivateKey, WorkingSet};
    use sov_rollup_interface::da::DaSpec;
    use sov_rollup_interface::services::da::DaService;
    use tokio::sync::watch;

    use super::batch_builder::EthBatchBuilder;
    #[cfg(feature = "local")]
    use super::DevSigner;
    use crate::gas_price::gas_oracle::GasPriceOracle;
    use crate::GasPriceOracleConfig;

    const ETH_RPC_ERROR: &str = "ETH_RPC_ERROR";
    const DEFAULT_CHAIN_ID: u64 = 1;

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
        rpc.register_async_method("eth_accounts", |_parameters, ethereum| async move {
            Ok::<_, ErrorObjectOwned>(ethereum.eth_signer.signers())
        })?;

        #[cfg(feature = "local")]
        rpc.register_async_method("eth_sendTransaction", |parameters, ethereum| async move {
            let mut transaction_request: TransactionRequest = parameters.one().unwrap();

            let evm = Evm::<S, Da::Spec>::default();

            // get from, return error if none
            let from = transaction_request
                .from
                .ok_or(to_jsonrpsee_error_object("No from address", ETH_RPC_ERROR))?;

            // return error if not in signers
            if !ethereum.eth_signer.signers().contains(&from) {
                return Err(to_jsonrpsee_error_object(
                    "From address not in signers",
                    ETH_RPC_ERROR,
                ));
            }

            let raw_evm_tx = {
                let mut working_set = WorkingSet::<S>::new(ethereum.storage.borrow().clone());

                // set nonce if none
                if transaction_request.nonce.is_none() {
                    let nonce = evm
                        .get_transaction_count(from, None, &mut working_set)
                        .unwrap_or_default();

                    transaction_request.nonce = Some(nonce);
                }

                let transaction =
                    to_typed_transaction_request(transaction_request, &evm, &mut working_set)?;

                // sign transaction
                let signed_tx = ethereum
                    .eth_signer
                    .sign_transaction(transaction, from)
                    .map_err(|e| to_jsonrpsee_error_object(e, ETH_RPC_ERROR))?;

                RlpEvmTransaction {
                    rlp: signed_tx.envelope_encoded().to_vec(),
                }
            };
            let (tx_hash, raw_message) = ethereum
                .make_raw_tx(raw_evm_tx)
                .map_err(|e| to_jsonrpsee_error_object(e, ETH_RPC_ERROR))?;

            ethereum.add_messages(vec![raw_message]);

            Ok::<_, ErrorObjectOwned>(tx_hash)
        })?;

        Ok(())
    }

    fn to_typed_transaction_request<S: sov_modules_api::Spec, Da: DaSpec>(
        transaction_request: TransactionRequest,
        evm: &Evm<S, Da>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<TypedTransactionRequest, ErrorObjectOwned> {
        let chain_id = evm
            .chain_id(working_set)
            .expect("Failed to get chain id")
            .map(|id| id.to())
            .unwrap_or(DEFAULT_CHAIN_ID);

        let gas_price = transaction_request.gas_price.unwrap_or_default();

        if transaction_request.from.is_none() {
            return Err(to_jsonrpsee_error_object("No from address", ETH_RPC_ERROR));
        }

        let estimated_gas = evm.eth_estimate_gas(
            TransactionRequest {
                from: transaction_request.from,
                to: transaction_request.to,
                gas: transaction_request.gas,
                gas_price: Some(gas_price),
                max_fee_per_gas: None,
                value: transaction_request.value,
                input: transaction_request.input.clone(),
                nonce: transaction_request.nonce,
                chain_id: Some(U64::from(chain_id)),
                access_list: transaction_request.access_list.clone(),
                max_priority_fee_per_gas: None,
                transaction_type: None,
                blob_versioned_hashes: None,
                max_fee_per_blob_gas: None,
                ..Default::default()
            },
            Some("pending".to_string()),
            working_set,
        )?;

        let gas_limit = estimated_gas.to::<U256>();

        let TransactionRequest {
            to,
            gas_price,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            gas,
            value,
            input: data,
            nonce,
            mut access_list,
            max_fee_per_blob_gas,
            blob_versioned_hashes,
            sidecar,
            ..
        } = transaction_request;

        let transaction = match (
            gas_price,
            max_fee_per_gas,
            access_list.take(),
            max_fee_per_blob_gas,
            blob_versioned_hashes,
            sidecar,
        ) {
            // legacy transaction
            // gas price required
            (Some(_), None, None, None, None, None) => {
                Some(TypedTransactionRequest::Legacy(LegacyTransactionRequest {
                    nonce: nonce.unwrap_or_default(),
                    gas_price: gas_price.unwrap_or_default(),
                    gas_limit: gas.unwrap_or_default(),
                    value: value.unwrap_or_default(),
                    input: data.into_input().unwrap_or_default(),
                    kind: address_to_tx_kind(to),
                    chain_id: None,
                }))
            }
            // EIP2930
            // if only access_list is set, and no eip1599 fees
            (_, None, Some(access_list), None, None, None) => Some(
                TypedTransactionRequest::EIP2930(EIP2930TransactionRequest {
                    nonce: nonce.unwrap_or_default(),
                    gas_price: gas_price.unwrap_or_default(),
                    gas_limit: gas.unwrap_or_default(),
                    value: value.unwrap_or_default(),
                    input: data.into_input().unwrap_or_default(),
                    kind: address_to_tx_kind(to),
                    chain_id: 0,
                    access_list,
                }),
            ),
            // EIP1559
            // if 4844 fields missing
            // gas_price, max_fee_per_gas, access_list,
            // max_fee_per_blob_gas, blob_versioned_hashes,
            // sidecar,
            (None, _, _, None, None, None) => {
                // Empty fields fall back to the canonical transaction schema.
                Some(TypedTransactionRequest::EIP1559(
                    EIP1559TransactionRequest {
                        nonce: nonce.unwrap_or_default(),
                        max_fee_per_gas: max_fee_per_gas.unwrap_or_default(),
                        max_priority_fee_per_gas: max_priority_fee_per_gas.unwrap_or_default(),
                        gas_limit: gas.unwrap_or_default(),
                        value: value.unwrap_or_default(),
                        input: data.into_input().unwrap_or_default(),
                        kind: address_to_tx_kind(to),
                        chain_id: 0,
                        access_list: access_list.unwrap_or_default(),
                    },
                ))
            }
            // EIP4884
            // all blob fields required
            (
                None,
                _,
                _,
                Some(max_fee_per_blob_gas),
                Some(blob_versioned_hashes),
                Some(sidecar),
            ) => {
                // As per the EIP, we follow the same semantics as EIP-1559.
                Some(TypedTransactionRequest::EIP4844(
                    EIP4844TransactionRequest {
                        chain_id: 0,
                        nonce: nonce.unwrap_or_default(),
                        max_priority_fee_per_gas: max_priority_fee_per_gas.unwrap_or_default(),
                        max_fee_per_gas: max_fee_per_gas.unwrap_or_default(),
                        gas_limit: gas.unwrap_or_default(),
                        value: value.unwrap_or_default(),
                        input: data.into_input().unwrap_or_default(),
                        kind: address_to_tx_kind(to),
                        access_list: access_list.unwrap_or_default(),

                        // eip-4844 specific.
                        max_fee_per_blob_gas,
                        blob_versioned_hashes,
                        sidecar,
                    },
                ))
            }

            _ => None,
        };

        Ok(match transaction {
            Some(TypedTransactionRequest::Legacy(mut m)) => {
                m.chain_id = Some(chain_id);
                m.gas_limit = gas_limit;
                m.gas_price = gas_price.unwrap_or_default();

                TypedTransactionRequest::Legacy(m)
            }
            Some(TypedTransactionRequest::EIP2930(mut m)) => {
                m.chain_id = chain_id;
                m.gas_limit = gas_limit;
                m.gas_price = gas_price.unwrap_or_default();

                TypedTransactionRequest::EIP2930(m)
            }
            Some(TypedTransactionRequest::EIP1559(mut m)) => {
                m.chain_id = chain_id;
                m.gas_limit = gas_limit;
                m.max_fee_per_gas = max_fee_per_gas.unwrap_or_default();

                TypedTransactionRequest::EIP1559(m)
            }
            Some(TypedTransactionRequest::EIP4844(mut m)) => {
                m.chain_id = chain_id;
                m.gas_limit = gas_limit;
                m.max_fee_per_gas = max_fee_per_gas.unwrap_or_default();

                TypedTransactionRequest::EIP4844(m)
            }
            None => return Err(EthApiError::ConflictingFeeFieldsInRequest.into()),
        })
    }

    fn address_to_tx_kind(to: Option<Address>) -> RpcTransactionKind {
        match to {
            Some(addr) => RpcTransactionKind::Call(addr),
            None => RpcTransactionKind::Create,
        }
    }
}
