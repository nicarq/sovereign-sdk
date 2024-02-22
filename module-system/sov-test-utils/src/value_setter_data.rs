use sov_modules_api::{CryptoSpec, Gas, PrivateKey, Spec};
use sov_value_setter::ValueSetter;

use super::*;
use crate::EncodeCall;

const DEFAULT_CHAIN_ID: u64 = 0;
const DEFAULT_GAS_TIP: u64 = 0;
const DEFAULT_GAS_LIMIT: u64 = 0;

pub struct ValueSetterMessage<S: Spec> {
    pub admin: Rc<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
    pub messages: Vec<u32>,
}

pub struct ValueSetterMessages<S: Spec> {
    pub messages: Vec<ValueSetterMessage<S>>,
}

impl<S: Spec> ValueSetterMessages<S> {
    pub fn new(messages: Vec<ValueSetterMessage<S>>) -> Self {
        Self { messages }
    }

    /// Returns a message containing one admin and two value setter messages.
    pub fn prepopulated() -> Self {
        Self::new(vec![ValueSetterMessage {
            admin: Rc::new(<<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate()),
            messages: vec![99, 33],
        }])
    }
}

impl<S: Spec> MessageGenerator for ValueSetterMessages<S> {
    type Module = ValueSetter<S>;
    type Spec = S;

    fn create_messages(&self) -> Vec<Message<Self::Spec, Self::Module>> {
        let mut messages = Vec::default();
        for value_setter_message in &self.messages {
            let admin = value_setter_message.admin.clone();

            for (value_setter_admin_nonce, new_value) in
                value_setter_message.messages.iter().enumerate()
            {
                let set_value_msg: sov_value_setter::CallMessage =
                    sov_value_setter::CallMessage::SetValue(*new_value);

                let max_gas_price = None;
                messages.push(Message::new(
                    admin.clone(),
                    set_value_msg,
                    DEFAULT_CHAIN_ID,
                    DEFAULT_GAS_TIP,
                    DEFAULT_GAS_LIMIT,
                    max_gas_price,
                    value_setter_admin_nonce.try_into().unwrap(),
                ));
            }
        }
        messages
    }

    fn create_tx<Encoder: EncodeCall<Self::Module>>(
        &self,
        sender: &<S::CryptoSpec as CryptoSpec>::PrivateKey,
        message: <Self::Module as Module>::CallMessage,
        chain_id: u64,
        gas_tip: u64,
        gas_limit: u64,
        max_gas_price: Option<<S::Gas as Gas>::Price>,
        nonce: u64,
        _is_last: bool,
    ) -> Transaction<S> {
        let message = Encoder::encode_call(message);
        Transaction::<S>::new_signed_tx(
            sender,
            message,
            chain_id,
            gas_tip,
            gas_limit,
            max_gas_price,
            nonce,
        )
    }
}
