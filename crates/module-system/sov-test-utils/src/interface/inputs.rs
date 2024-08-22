use std::collections::HashMap;

use sov_modules_api::transaction::{PriorityFeeBips, Transaction, TxDetails, UnsignedTransaction};
use sov_modules_api::{ApiStateAccessor, CryptoSpec, EncodeCall, Module, PrivateKey, RawTx, Spec};

use crate::FromState;

/// Defines the type of a message that can be sent to the runtime.
pub enum TransactionType<M: Module, S: Spec> {
    /// A pre-signed transaction. Ie, a transaction that has already been signed and formatted by the sender
    PreSigned(RawTx),
    /// A pre-encoded transaction. That is a transaction that has not been signed yet, but has been encoded for the module system
    PreEncoded {
        /// A pre-encoded message to be sent. This has been signed by a given runtime configuration.
        encoded_message: Vec<u8>,
        /// The private key of the sender.
        key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
        /// The details of the transaction.
        details: TxDetails<S>,
    },
    /// A plain transaction. That is a transaction that has not been signed or encoded yet
    Plain {
        /// A plain call message to be sent.
        message: M::CallMessage,
        /// The private key of the sender.
        key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
        /// The details of the transaction.
        details: TxDetails<S>,
    },
    /// A message type that needs to be configured before it can be sent
    Configuration {
        /// A plain message to be sent to the state.
        message: Box<dyn FromState<S, Output = M::CallMessage>>,
        /// The private key of the sender.
        key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
        /// The details of the transaction.
        details: TxDetails<S>,
    },
}

impl<M: Module, S: Spec> TransactionType<M, S> {
    fn details_mut(&mut self) -> Option<&mut TxDetails<S>> {
        Some(match self {
            TransactionType::PreSigned { .. } => return None,
            TransactionType::Plain { details, .. }
            | TransactionType::PreEncoded { details, .. }
            | TransactionType::Configuration { details, .. } => details,
        })
    }

    /// Override the details of the transaction. This method panics if called with [`TransactionType::PreSigned`].
    pub fn with_details(self, details: TxDetails<S>) -> Self {
        match self {
            TransactionType::Plain { message, key, .. } => TransactionType::Plain {
                message,
                key,
                details,
            },
            TransactionType::PreEncoded {
                encoded_message,
                key,
                ..
            } => TransactionType::PreEncoded {
                encoded_message,
                key,
                details,
            },
            TransactionType::Configuration { message, key, .. } => TransactionType::Configuration {
                message,
                key,
                details,
            },
            TransactionType::PreSigned(_) => {
                panic!("PreSigned transactions cannot specify custom details")
            }
        }
    }

    /// Set the chain ID of the transaction.
    pub fn with_chain_id(mut self, chain_id: u64) -> Self {
        if let Some(details) = self.details_mut() {
            details.chain_id = chain_id;
        }

        self
    }

    /// Set the max priority fee of the transaction.
    pub fn with_max_priority_fee_bips(mut self, max_priority_fee_bips: PriorityFeeBips) -> Self {
        if let Some(details) = self.details_mut() {
            details.max_priority_fee_bips = max_priority_fee_bips;
        }

        self
    }

    /// Set the max fee of the transaction.
    pub fn with_max_fee(mut self, max_fee: u64) -> Self {
        if let Some(details) = self.details_mut() {
            details.max_fee = max_fee;
        }

        self
    }

    /// Set the gas limit of the transaction.
    pub fn with_gas_limit(mut self, gas_limit: Option<S::Gas>) -> Self {
        if let Some(details) = self.details_mut() {
            details.gas_limit = gas_limit;
        }

        self
    }

    /// Converts a [`TransactionType`] into a [`RawTx`].
    pub fn to_raw_tx<RT: EncodeCall<M>>(
        self,
        nonces: &mut HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
        state: &mut ApiStateAccessor<S>,
    ) -> RawTx {
        match self {
            TransactionType::PreSigned(raw_tx) => raw_tx,
            TransactionType::PreEncoded {
                encoded_message,
                key,
                details,
            } => Self::sign(encoded_message, key, details, nonces),
            TransactionType::Plain {
                message,
                key,
                details,
            } => {
                let msg = <RT as EncodeCall<M>>::encode_call(message);
                Self::sign(msg, key, details, nonces)
            }
            TransactionType::Configuration {
                message,
                key,
                details,
            } => {
                let msg = message.from_state(state);
                let msg = <RT as EncodeCall<M>>::encode_call(msg);
                Self::sign(msg, key, details, nonces)
            }
        }
    }

    /// Creates a [`TransactionType`] from a [`UnsignedTransaction`].
    pub fn pre_signed(
        unsigned_tx: UnsignedTransaction<S>,
        key: &<S::CryptoSpec as CryptoSpec>::PrivateKey,
    ) -> Self {
        let tx = borsh::to_vec(&Transaction::new_signed_tx(key, unsigned_tx)).unwrap();
        Self::PreSigned(RawTx { data: tx })
    }

    fn sign(
        msg: Vec<u8>,
        key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
        details: TxDetails<S>,
        nonces: &mut HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    ) -> RawTx {
        let pub_key = key.pub_key();
        let nonce = *nonces.get(&pub_key).unwrap_or(&0);
        nonces.insert(pub_key, nonce + 1);
        let tx = borsh::to_vec(&Transaction::<S>::new_signed_tx(
            &key,
            UnsignedTransaction::new(
                msg,
                details.chain_id,
                details.max_priority_fee_bips,
                details.max_fee,
                nonce,
                details.gas_limit,
            ),
        ))
        .unwrap();

        RawTx { data: tx }
    }
}

/// Defines the type of batch that can be sent to the runtime.
pub struct BatchType<M: Module, S: Spec>(pub Vec<TransactionType<M, S>>);

impl<S: Spec, M: Module> From<Vec<TransactionType<M, S>>> for BatchType<M, S> {
    fn from(value: Vec<TransactionType<M, S>>) -> Self {
        Self(value)
    }
}

/// Defines the type of proof that can be sent to the runtime.
pub enum ProofType<S: Spec> {
    /// Provide the proof upfront as a byte array.
    Inline(Vec<u8>),
    /// Derive the proof at a later point in time utilizing the current rollup state.
    Configuration(Box<dyn FromState<S, Output = Vec<u8>>>),
}

/// Input that can be executed in a slot ran by the test runtime.
#[derive(derive_more::From)]
pub enum SlotInput<S: Spec, M: Module> {
    /// Execute a transaction as input to a slot.
    Transaction(TransactionType<M, S>),
    /// Execute a batch as input to a slot.
    Batch(BatchType<M, S>),
    /// Execute a proof as input to a slot.
    Proof(ProofType<S>),
}
