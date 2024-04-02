use std::rc::Rc;

use borsh::ser::BorshSerialize;
use sov_mock_da::verifier::MockDaSpec;
use sov_mock_da::{MockAddress, MockBlob};
pub use sov_mock_zkvm::MockZkVerifier;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::transaction::Transaction;
pub use sov_modules_api::EncodeCall;
use sov_modules_api::{CryptoSpec, DaSpec, Gas, Module, RollupAddress, Spec};
use sov_modules_stf_blueprint::{Batch, BatchReceipt, RawTx, TxEffect};

pub mod attester_incentive_data;
pub mod bank_data;
mod evm;
pub mod logging;
pub mod runtime;
pub mod value_setter_data;

pub use evm::simple_smart_contract::SimpleStorageContract;

pub type TestSpec = sov_modules_api::default_spec::DefaultSpec<MockZkVerifier, MockZkVerifier>;
pub type ZkTestSpec = sov_modules_api::default_spec::ZkDefaultSpec<MockZkVerifier, MockZkVerifier>;
pub type TestPrivateKey = <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;
pub type TestPublicKey = <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PublicKey;
pub type TestSignature = <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::Signature;

pub fn new_test_blob_from_batch(
    batch: BatchWithId,
    address: &[u8],
    hash: [u8; 32],
) -> <MockDaSpec as DaSpec>::BlobTransaction {
    let address = MockAddress::try_from(address).unwrap();
    let data = Batch { txs: batch.txs }.try_to_vec().unwrap();
    MockBlob::new(data, address, hash)
}

pub fn has_tx_events<A: RollupAddress>(
    apply_blob_outcome: &BatchReceipt<sov_modules_stf_blueprint::SequencerOutcome<A>, TxEffect>,
) -> bool {
    let events = apply_blob_outcome
        .tx_receipts
        .iter()
        .flat_map(|receipts| receipts.events.iter());

    events.peekable().peek().is_some()
}

/// A generic message object used to create transactions.
pub struct Message<S: Spec, Mod: Module> {
    /// The sender's private key.
    pub sender_key: Rc<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
    /// The message content.
    pub content: Mod::CallMessage,
    /// The ID of the chain.
    pub chain_id: u64,
    /// The gas tip for the sequencer.
    pub gas_tip: u64,
    /// The gas limit for the transaction execution.
    pub gas_limit: u64,
    /// The maximum gas price for the transaction execution.
    pub max_gas_price: Option<<S::Gas as Gas>::Price>,
    /// The message nonce.
    pub nonce: u64,
}

impl<S: Spec, Mod: Module> Message<S, Mod> {
    fn new(
        sender_key: Rc<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
        content: Mod::CallMessage,
        chain_id: u64,
        gas_tip: u64,
        gas_limit: u64,
        max_gas_price: Option<<S::Gas as Gas>::Price>,
        nonce: u64,
    ) -> Self {
        Self {
            sender_key,
            content,
            chain_id,
            gas_tip,
            gas_limit,
            max_gas_price,
            nonce,
        }
    }

    pub fn to_tx<Encoder: EncodeCall<Mod>>(self) -> sov_modules_api::transaction::Transaction<S> {
        let message = Encoder::encode_call(self.content);
        Transaction::<S>::new_signed_tx(
            &self.sender_key,
            message,
            self.chain_id,
            self.gas_tip,
            self.gas_limit,
            self.max_gas_price,
            self.nonce,
        )
    }
}

/// Trait used to generate messages from the DA layer to automate module testing
pub trait MessageGenerator {
    const DEFAULT_CHAIN_ID: u64 = 0;
    const DEFAULT_GAS_TIP: u64 = 0;
    const DEFAULT_GAS_LIMIT: u64 = 100;
    const DEFAULT_MAX_GAS_PRICE: [u64; 2] = [1, 1];

    /// Module where the messages originate from.
    type Module: Module;

    /// Module spec
    type Spec: Spec;

    /// Generates a list of messages originating from the module.
    fn create_messages(&self) -> Vec<Message<Self::Spec, Self::Module>>;

    /// Creates a vector of raw transactions from the module.
    fn create_raw_txs<Encoder: EncodeCall<Self::Module>>(&self) -> Vec<RawTx> {
        let messages_iter = self.create_messages().into_iter().peekable();
        let mut serialized_messages = Vec::default();
        for message in messages_iter {
            let tx = message.to_tx::<Encoder>();
            serialized_messages.push(RawTx {
                data: tx.try_to_vec().unwrap(),
            });
        }
        serialized_messages
    }

    /// Creates a vector of raw transactions from the module.
    fn create_raw_txs_with_maximum_gas_price<Encoder: EncodeCall<Self::Module>>(
        &self,
        max_gas_price: <<Self::Spec as Spec>::Gas as Gas>::Price,
    ) -> Vec<RawTx> {
        let messages_iter = self.create_messages().into_iter().peekable();
        let mut serialized_messages = Vec::default();
        for mut message in messages_iter {
            message.max_gas_price.replace(max_gas_price.clone());
            serialized_messages.push(RawTx {
                data: message.to_tx::<Encoder>().try_to_vec().unwrap(),
            });
        }
        serialized_messages
    }

    fn create_blobs<Encoder: EncodeCall<Self::Module>>(&self) -> Vec<u8> {
        let txs: Vec<Vec<u8>> = self
            .create_raw_txs::<Encoder>()
            .into_iter()
            .map(|tx| tx.data)
            .collect();

        txs.try_to_vec().unwrap()
    }
}
