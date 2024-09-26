//! Several utilities needed to generate message for testing.
//!
//! TODO: Add a doctest to describe how to generate messages in tests.

use std::rc::Rc;

use sov_modules_api::capabilities::TransactionAuthenticator;
use sov_modules_api::macros::config_value;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, TxDetails, UnsignedTransaction};
use sov_modules_api::{Batch, CryptoSpec, EncodeCall, FullyBakedTx, Module, RawTx, Spec};

use crate::{TEST_DEFAULT_GAS_LIMIT, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE};

/// Utilities for generating messages for the bank module.
pub mod bank;
/// Utilities for generating messages for the sequencer registry module.
pub mod sequencer_registry;
/// Utilities for generating messages for the value setter module.
pub mod value_setter;

/// A generic message object used to create transactions.
pub struct Message<S: Spec, Mod: Module> {
    /// The sender's private key.
    pub sender_key: Rc<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
    /// The message content.
    pub content: Mod::CallMessage,
    /// Data related to fees and gas handling.
    pub details: TxDetails<S>,
    /// The message nonce.
    pub nonce: u64,
}

impl<S: Spec, Mod: Module> Message<S, Mod> {
    fn new(
        sender_key: Rc<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
        content: Mod::CallMessage,
        chain_id: u64,
        max_priority_fee_bips: PriorityFeeBips,
        max_fee: u64,
        gas_limit: Option<S::Gas>,
        nonce: u64,
    ) -> Self {
        Self {
            sender_key,
            content,
            details: TxDetails {
                chain_id,
                max_priority_fee_bips,
                max_fee,
                gas_limit,
            },
            nonce,
        }
    }

    /// Converts a [`Message`] into a [`Transaction`] using the [`TxDetails`] provided by the [`Message`].
    pub fn to_tx<Encoder: EncodeCall<Mod>>(self) -> sov_modules_api::transaction::Transaction<S> {
        let message = Encoder::encode_call(self.content);
        Transaction::<S>::new_signed_tx(
            &self.sender_key,
            UnsignedTransaction::new(
                message,
                self.details.chain_id,
                self.details.max_priority_fee_bips,
                self.details.max_fee,
                self.nonce,
                self.details.gas_limit,
            ),
        )
    }
}

/// Trait used to generate messages from the DA layer to automate module testing
pub trait MessageGenerator {
    /// The default chain ID to use for the messages.
    const DEFAULT_CHAIN_ID: u64 = config_value!("CHAIN_ID");

    /// Module where the messages originate from.
    type Module: Module;

    /// Module spec
    type Spec: Spec;

    /// Generates a list of messages originating from the module using the provided transaction details.
    fn create_messages(
        &self,
        chain_id: u64,
        max_priority_fee_bips: PriorityFeeBips,
        max_fee: u64,
        estimated_gas_usage: Option<<Self::Spec as Spec>::Gas>,
    ) -> Vec<Message<Self::Spec, Self::Module>>;

    /// Generates a list of messages originating from the module using default transaction details.
    /// Note: sets the gas usage to the default gas limit.
    fn create_default_messages(&self) -> Vec<Message<Self::Spec, Self::Module>> {
        self.create_messages(
            Self::DEFAULT_CHAIN_ID,
            TEST_DEFAULT_MAX_PRIORITY_FEE,
            TEST_DEFAULT_MAX_FEE,
            Some(<Self::Spec as Spec>::Gas::from(TEST_DEFAULT_GAS_LIMIT)),
        )
    }

    /// Generates a list of messages originating from the module using default transaction details and no gas usage.
    fn create_default_messages_without_gas_usage(&self) -> Vec<Message<Self::Spec, Self::Module>> {
        self.create_messages(
            Self::DEFAULT_CHAIN_ID,
            TEST_DEFAULT_MAX_PRIORITY_FEE,
            TEST_DEFAULT_MAX_FEE,
            None,
        )
    }

    /// Creates a vector of raw transactions from the module.
    fn create_default_encoded_txs<
        RT: TransactionAuthenticator<Self::Spec> + EncodeCall<Self::Module>,
    >(
        &self,
    ) -> Vec<FullyBakedTx> {
        self.create_encoded_txs::<RT>(
            Self::DEFAULT_CHAIN_ID,
            TEST_DEFAULT_MAX_PRIORITY_FEE,
            TEST_DEFAULT_MAX_FEE,
            Some(<Self::Spec as Spec>::Gas::from(TEST_DEFAULT_GAS_LIMIT)),
        )
    }

    /// Generates a list of raw transactions originating from the module using default transaction details and no gas usage.
    fn create_default_encoded_txs_without_gas_usage<
        RT: TransactionAuthenticator<Self::Spec> + EncodeCall<Self::Module>,
    >(
        &self,
    ) -> Vec<FullyBakedTx> {
        self.create_encoded_txs::<RT>(
            Self::DEFAULT_CHAIN_ID,
            TEST_DEFAULT_MAX_PRIORITY_FEE,
            TEST_DEFAULT_MAX_FEE,
            None,
        )
    }

    /// Creates a vector of raw transactions from the module.
    fn create_encoded_txs<RT: TransactionAuthenticator<Self::Spec> + EncodeCall<Self::Module>>(
        &self,
        chain_id: u64,
        max_priority_fee_bips: PriorityFeeBips,
        max_fee: u64,
        estimated_gas_usage: Option<<Self::Spec as Spec>::Gas>,
    ) -> Vec<FullyBakedTx> {
        let messages_iter = self
            .create_messages(
                chain_id,
                max_priority_fee_bips,
                max_fee,
                estimated_gas_usage,
            )
            .into_iter();
        let mut serialized_messages = Vec::default();
        for message in messages_iter {
            let tx = message.to_tx::<RT>();
            serialized_messages.push(RT::encode_with_standard_auth(RawTx::new(
                borsh::to_vec(&tx).unwrap(),
            )));
        }
        serialized_messages
    }

    /// Generates a list of blobs originating from the module using default transaction details.
    /// This function calls [`MessageGenerator::create_default_encoded_txs`] and then wraps the resulting vec of [`RawTx`]s into a [`Batch`].
    fn create_blobs<RT: TransactionAuthenticator<Self::Spec> + EncodeCall<Self::Module>>(
        &self,
    ) -> Vec<u8> {
        let txs: Vec<FullyBakedTx> = self
            .create_default_encoded_txs::<RT>()
            .into_iter()
            .collect();

        let batch = Batch::new(txs);

        borsh::to_vec(&batch).unwrap()
    }
}
