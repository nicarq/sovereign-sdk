use std::borrow::BorrowMut;
use std::collections::VecDeque;

use borsh::BorshSerialize;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::CryptoSpec;

pub struct EthBatchBuilder<S: sov_modules_api::Spec> {
    mempool: VecDeque<Vec<u8>>,
    sov_tx_signer_private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    nonce: u64,
    min_blob_size: Option<usize>,
}

impl<S: sov_modules_api::Spec> EthBatchBuilder<S> {
    /// Creates a new `EthBatchBuilder`.
    pub fn new(
        sov_tx_signer_private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
        nonce: u64,
        min_blob_size: Option<usize>,
    ) -> Self {
        EthBatchBuilder {
            mempool: VecDeque::new(),
            sov_tx_signer_private_key,
            nonce,
            min_blob_size,
        }
    }

    /// Signs messages with the private key of the `EthBatchBuilder` and make them `transactions`.
    /// Returns the blob of signed transactions.
    fn make_blob(&mut self) -> Vec<Vec<u8>> {
        let mut txs = Vec::new();

        let nonce = self.nonce.borrow_mut();

        while let Some(raw_message) = self.mempool.pop_front() {
            // TODO define a strategy to expose chain id and gas tip for ethereum frontend
            let chain_id = 0;
            let gas_tip = 0;
            let gas_limit = 0;
            let max_gas_price = None;

            let raw_tx = Transaction::<S>::new_signed_tx(
                &self.sov_tx_signer_private_key,
                raw_message,
                chain_id,
                gas_tip,
                gas_limit,
                max_gas_price,
                *nonce,
            )
            .try_to_vec()
            .unwrap();

            *nonce += 1;

            txs.push(raw_tx);
        }
        txs
    }

    /// Adds `messages` to the mempool.
    pub fn add_messages(&mut self, messages: Vec<Vec<u8>>) {
        for message in messages {
            self.mempool.push_back(message);
        }
    }

    /// Attempts to create a blob with a minimum size of `min_blob_size`.
    pub fn get_next_blob(&mut self, min_blob_size: Option<usize>) -> Vec<Vec<u8>> {
        let min_blob_size = min_blob_size.or(self.min_blob_size);

        if let Some(min_blob_size) = min_blob_size {
            if self.mempool.len() >= min_blob_size {
                return self.make_blob();
            }
        }
        Vec::default()
    }

    /// Adds `messages` to the mempool and attempts to create a blob with a minimum size of `min_blob_size`.
    pub fn add_messages_and_get_next_blob(
        &mut self,
        min_blob_size: Option<usize>,
        messages: Vec<Vec<u8>>,
    ) -> Vec<Vec<u8>> {
        self.add_messages(messages);
        self.get_next_blob(min_blob_size)
    }
}
