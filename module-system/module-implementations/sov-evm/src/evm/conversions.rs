use reth_primitives::{
    Bytes as RethBytes, TransactionSigned, TransactionSignedEcRecovered, TransactionSignedNoHash,
};
use revm::primitives::{CreateScheme, TransactTo, TxEnv, U256};
use revm_primitives::BlockEnv;

use super::primitive_types::{RlpEvmTransaction, SealedBlock};
use crate::error::rpc::EthApiError;

// BlockEnv from SealedBlock
impl From<SealedBlock> for BlockEnv {
    fn from(block: SealedBlock) -> Self {
        Self {
            number: U256::from(block.header.number),
            coinbase: block.header.beneficiary,
            timestamp: U256::from(block.header.timestamp),
            prevrandao: Some(block.header.mix_hash),
            basefee: block.header.base_fee_per_gas.map_or(U256::ZERO, U256::from),
            gas_limit: U256::from(block.header.gas_limit),
            // Not used fields:
            blob_excess_gas_and_price: None,
            difficulty: Default::default(),
        }
    }
}

pub(crate) fn create_tx_env(tx: &TransactionSignedEcRecovered) -> TxEnv {
    let to = match tx.to() {
        Some(addr) => TransactTo::Call(addr),
        None => TransactTo::Create(CreateScheme::Create),
    };

    TxEnv {
        caller: tx.signer(),
        gas_limit: tx.gas_limit(),
        gas_price: U256::from(tx.effective_gas_price(None)),
        gas_priority_fee: tx.max_priority_fee_per_gas().map(U256::from),
        transact_to: to,
        value: tx.value(),
        data: tx.input().clone(),
        chain_id: tx.chain_id(),
        nonce: Some(tx.nonce()),
        // TODO handle access list
        access_list: vec![],
        // EIP-4844 related fields
        blob_hashes: Default::default(),
        max_fee_per_blob_gas: None,
    }
}

impl TryFrom<RlpEvmTransaction> for TransactionSignedNoHash {
    type Error = EthApiError;

    fn try_from(data: RlpEvmTransaction) -> Result<Self, Self::Error> {
        let data = RethBytes::from(data.rlp);
        if data.is_empty() {
            return Err(EthApiError::EmptyRawTransactionData);
        }

        let transaction = TransactionSigned::decode_enveloped(&mut data.as_ref())
            .map_err(|_| EthApiError::FailedToDecodeSignedTransaction)?;

        Ok(transaction.into())
    }
}

impl TryFrom<RlpEvmTransaction> for TransactionSignedEcRecovered {
    type Error = EthApiError;

    fn try_from(evm_tx: RlpEvmTransaction) -> Result<Self, Self::Error> {
        let tx = TransactionSignedNoHash::try_from(evm_tx)?;
        let tx: TransactionSigned = tx.into();
        let tx = tx
            .into_ecrecovered()
            .ok_or(EthApiError::FailedToDecodeSignedTransaction)?;

        Ok(tx)
    }
}
