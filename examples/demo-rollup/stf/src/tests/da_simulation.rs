use std::rc::Rc;

use borsh::BorshSerialize;
use sov_bank::Bank;
use sov_mock_da::MockDaSpec;
use sov_modules_api::runtime::capabilities::RawTx;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{EncodeCall, PrivateKey, Spec};
use sov_test_utils::bank_data::BankMessageGenerator;
use sov_test_utils::value_setter_data::{ValueSetterMessage, ValueSetterMessages};
use sov_test_utils::{MessageGenerator, TestPrivateKey};

use crate::runtime::Runtime;

pub(crate) type S = sov_test_utils::TestSpec;
type Da = MockDaSpec;

pub fn simulate_da(admin: TestPrivateKey) -> Vec<RawTx> {
    let mut messages = Vec::default();

    let bank_generator = BankMessageGenerator::<S>::with_minter_and_transfer(admin.clone());
    let bank_messages = bank_generator.create_messages();

    let value_setter = ValueSetterMessages::new(vec![ValueSetterMessage {
        admin: Rc::new(admin),
        messages: vec![99, 33],
    }]);
    messages.extend(value_setter.create_raw_txs::<Runtime<S, Da>>());
    let nonce_offset = messages.len() as u64;
    for mut msg in bank_messages {
        msg.nonce += nonce_offset;
        let tx = msg.to_tx::<Runtime<S, Da>>();
        messages.push(RawTx {
            data: tx.try_to_vec().unwrap(),
        });
    }
    messages
}

/// TODO(@theochap): This allow(dead_code) is only temporary and will be removed once the test `test_tx_gas_limit` is fixed.
/// `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/398>`
#[allow(dead_code)]
pub fn simulate_da_with_gas_limit(
    value_setter_admin: TestPrivateKey,
    gas_limit: <S as Spec>::Gas,
) -> Vec<RawTx> {
    let mut messages = Vec::default();

    let value_setter = ValueSetterMessages::new(vec![ValueSetterMessage {
        admin: Rc::new(value_setter_admin),
        messages: vec![99, 33],
    }]);

    let txs = value_setter.create_raw_txs_with_maximum_gas_price::<Runtime<S, Da>>(gas_limit);
    messages.extend(txs);
    messages
}

pub fn simulate_da_with_revert_msg(admin: TestPrivateKey) -> Vec<RawTx> {
    let mut messages = Vec::default();
    let bank_generator = BankMessageGenerator::<S>::create_invalid_transfer(admin);
    let bank_txns = bank_generator.create_raw_txs::<Runtime<S, Da>>();
    messages.extend(bank_txns);
    messages
}

pub fn simulate_da_with_bad_sig(key: TestPrivateKey) -> Vec<RawTx> {
    let b: BankMessageGenerator<S> = BankMessageGenerator::with_minter(key.clone());
    let create_token_message = b.create_messages().remove(0);
    let tx = Transaction::<S>::new(
        create_token_message.sender_key.pub_key(),
        <Runtime<S, Da> as EncodeCall<Bank<S>>>::encode_call(create_token_message.content.clone()),
        // Use the signature of an empty message
        key.sign(&[]),
        create_token_message.chain_id,
        create_token_message.max_priority_fee,
        create_token_message.max_fee,
        create_token_message.gas_limit,
        create_token_message.nonce,
    );
    // Overwrite the signature with the signature of the empty message

    vec![RawTx {
        data: tx.try_to_vec().unwrap(),
    }]
}

pub fn simulate_da_with_bad_nonce(key: TestPrivateKey) -> Vec<RawTx> {
    let b: BankMessageGenerator<S> = BankMessageGenerator::with_minter(key);
    let mut create_token_message = b.create_messages().remove(0);
    // Overwrite the nonce with the maximum value
    create_token_message.nonce = u64::MAX;
    let tx = create_token_message.to_tx::<Runtime<S, Da>>();
    vec![RawTx {
        data: tx.try_to_vec().unwrap(),
    }]
}

pub fn simulate_da_with_bad_serialization(key: TestPrivateKey) -> Vec<RawTx> {
    let b: BankMessageGenerator<S> = BankMessageGenerator::with_minter(key);
    let create_token_message = b.create_messages().remove(0);
    let tx = Transaction::<S>::new_signed_tx(
        &create_token_message.sender_key,
        b"not a real call message".to_vec(),
        create_token_message.chain_id,
        create_token_message.max_priority_fee,
        create_token_message.max_fee,
        create_token_message.gas_limit,
        create_token_message.nonce,
    );

    vec![RawTx {
        data: tx.try_to_vec().unwrap(),
    }]
}
