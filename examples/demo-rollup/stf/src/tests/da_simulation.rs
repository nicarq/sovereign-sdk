use std::rc::Rc;

use sov_mock_da::MockDaSpec;
use sov_modules_api::default_context::DefaultContext;
use sov_modules_api::default_signature::private_key::DefaultPrivateKey;
use sov_modules_api::{Context, Gas};
use sov_modules_stf_blueprint::RawTx;
use sov_test_utils::bank_data::{
    BadNonceBankCallMessages, BadSerializationBankCallMessages, BadSignatureBankCallMessages,
    BankMessageGenerator,
};
use sov_test_utils::value_setter_data::{ValueSetterMessage, ValueSetterMessages};
use sov_test_utils::MessageGenerator;

use crate::runtime::Runtime;

type C = DefaultContext;
type Da = MockDaSpec;

pub fn simulate_da(value_setter_admin: DefaultPrivateKey) -> Vec<RawTx> {
    let mut messages = Vec::default();

    let bank_generator = BankMessageGenerator::<C>::default();
    let bank_txs = bank_generator.create_raw_txs::<Runtime<C, Da>>();

    let value_setter = ValueSetterMessages::new(vec![ValueSetterMessage {
        admin: Rc::new(value_setter_admin),
        messages: vec![99, 33],
    }]);
    messages.extend(value_setter.create_raw_txs::<Runtime<C, Da>>());
    messages.extend(bank_txs);
    messages
}

pub fn simulate_da_with_max_gas_price(
    value_setter_admin: DefaultPrivateKey,
    max_gas_price: <<DefaultContext as Context>::Gas as Gas>::Price,
) -> Vec<RawTx> {
    let mut messages = Vec::default();

    let value_setter = ValueSetterMessages::new(vec![ValueSetterMessage {
        admin: Rc::new(value_setter_admin),
        messages: vec![99, 33],
    }]);

    let txs = value_setter.create_raw_txs_with_maximum_gas_price::<Runtime<C, Da>>(max_gas_price);
    messages.extend(txs);
    messages
}

pub fn simulate_da_with_revert_msg() -> Vec<RawTx> {
    let mut messages = Vec::default();
    let bank_generator = BankMessageGenerator::<C>::create_invalid_transfer();
    let bank_txns = bank_generator.create_raw_txs::<Runtime<C, Da>>();
    messages.extend(bank_txns);
    messages
}

pub fn simulate_da_with_bad_sig() -> Vec<RawTx> {
    let b: BadSignatureBankCallMessages = Default::default();
    b.create_raw_txs::<Runtime<C, Da>>()
}

pub fn simulate_da_with_bad_nonce() -> Vec<RawTx> {
    let b: BadNonceBankCallMessages = Default::default();
    b.create_raw_txs::<Runtime<C, Da>>()
}

pub fn simulate_da_with_bad_serialization() -> Vec<RawTx> {
    let b: BadSerializationBankCallMessages = Default::default();
    b.create_raw_txs::<Runtime<C, Da>>()
}
