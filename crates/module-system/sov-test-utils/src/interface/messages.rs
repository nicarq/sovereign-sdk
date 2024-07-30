use std::collections::HashMap;

use sov_modules_api::macros::config_value;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{ApiStateAccessor, CryptoSpec, EncodeCall, Module, PrivateKey, RawTx, Spec};

use crate::{TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE};

/// A special configuration trait for messages that need to be configured before they can be sent.
pub trait IntoCallMessage<M: Module, S: Spec> {
    /// Executes the configuration logic and returns the associated call message.
    fn into_call_message(self: Box<Self>, state: &mut ApiStateAccessor<S>) -> M::CallMessage;
}

/// Defines the type of a message that can be sent to the runtime.
pub enum MessageType<M: Module, S: Spec> {
    /// A pre-signed transaction. Ie, a transaction that has already been signed and formatted by the sender
    PreSigned(RawTx),
    /// A pre-encoded transaction. That is a transaction that has not been signed yet, but has been encoded for the module system
    PreEncoded(Vec<u8>, <S::CryptoSpec as CryptoSpec>::PrivateKey),
    /// A plain transaction. That is a transaction that has not been signed or encoded yet
    Plain(M::CallMessage, <S::CryptoSpec as CryptoSpec>::PrivateKey),
    /// A message type that needs to be configured before it can be sent
    Configuration(
        Box<dyn IntoCallMessage<M, S>>,
        <S::CryptoSpec as CryptoSpec>::PrivateKey,
    ),
}

impl<M: Module, S: Spec> MessageType<M, S> {
    /// Converts a [`MessageType`] into a [`RawTx`].
    pub fn to_raw_tx<RT: EncodeCall<M>>(
        self,
        nonces: &mut HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
        state: &mut ApiStateAccessor<S>,
    ) -> RawTx {
        match self {
            MessageType::PreSigned(raw_tx) => raw_tx,
            MessageType::PreEncoded(msg, key) => Self::sign_with_defaults(msg, key, nonces),
            MessageType::Plain(msg, key) => {
                let msg = <RT as EncodeCall<M>>::encode_call(msg);
                Self::sign_with_defaults(msg, key, nonces)
            }
            MessageType::Configuration(msg, key) => {
                let msg = msg.into_call_message(state);
                let msg = <RT as EncodeCall<M>>::encode_call(msg);
                Self::sign_with_defaults(msg, key, nonces)
            }
        }
    }

    /// Creates a [`MessageType`] from a [`UnsignedTransaction`].
    pub fn pre_signed(
        unsigned_tx: UnsignedTransaction<S>,
        key: &<S::CryptoSpec as CryptoSpec>::PrivateKey,
    ) -> Self {
        let tx = borsh::to_vec(&Transaction::new_signed_tx(key, unsigned_tx)).unwrap();
        Self::PreSigned(RawTx { data: tx })
    }

    fn sign_with_defaults(
        msg: Vec<u8>,
        key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
        nonces: &mut HashMap<<S::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    ) -> RawTx {
        let pub_key = key.pub_key();
        let nonce = *nonces.get(&pub_key).unwrap_or(&0);
        nonces.insert(pub_key, nonce + 1);
        let tx = borsh::to_vec(&Transaction::<S>::new_signed_tx(
            &key,
            UnsignedTransaction::new(
                msg,
                config_value!("CHAIN_ID"),
                TEST_DEFAULT_MAX_PRIORITY_FEE,
                TEST_DEFAULT_MAX_FEE,
                nonce,
                None,
            ),
        ))
        .unwrap();

        RawTx { data: tx }
    }
}
