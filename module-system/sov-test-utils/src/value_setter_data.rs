use sov_modules_api::{CryptoSpec, GasArray, PrivateKey, Spec};
use sov_value_setter::ValueSetter;

use super::*;

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

                messages.push(Message::new(
                    admin.clone(),
                    set_value_msg,
                    Self::DEFAULT_CHAIN_ID,
                    Self::DEFAULT_GAS_TIP,
                    Self::DEFAULT_GAS_LIMIT,
                    Some(<<Self::Spec as Spec>::Gas as Gas>::Price::from_slice(
                        &Self::DEFAULT_MAX_GAS_PRICE,
                    )),
                    value_setter_admin_nonce.try_into().unwrap(),
                ));
            }
        }
        messages
    }
}
