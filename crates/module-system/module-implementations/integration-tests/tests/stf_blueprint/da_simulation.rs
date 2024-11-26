use std::rc::Rc;

use sov_bank::Bank;
use sov_modules_api::capabilities::TransactionAuthenticator;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{EncodeCall, FullyBakedTx, PrivateKey, RawTx, Runtime};
use sov_test_utils::generators::bank::BankMessageGenerator;
use sov_test_utils::generators::sequencer_registry::SequencerRegistryMessageGenerator;
use sov_test_utils::generators::value_setter::{ValueSetterMessage, ValueSetterMessages};
use sov_test_utils::{MessageGenerator, TestPrivateKey};

use super::IntegTestRuntime;

pub(crate) type S = sov_test_utils::TestSpec;

pub fn simulate_da(admin: TestPrivateKey) -> Vec<FullyBakedTx> {
    let mut messages = Vec::default();

    let bank_generator = BankMessageGenerator::<S>::with_minter_and_transfer(admin.clone());
    let bank_messages = bank_generator.create_default_messages_without_gas_usage();

    let value_setter = ValueSetterMessages::new(vec![ValueSetterMessage {
        admin: Rc::new(admin),
        messages: vec![99, 33],
    }]);
    messages
        .extend(value_setter.create_default_encoded_txs_without_gas_usage::<IntegTestRuntime<S>>());
    let nonce_offset = messages.len() as u64;
    for mut msg in bank_messages {
        msg.nonce += nonce_offset;
        let tx = msg.to_tx::<IntegTestRuntime<S>>();
        messages.push(encode_with_auth(tx));
    }
    messages
}

pub fn simulate_da_with_revert_msg(admin: TestPrivateKey) -> Vec<FullyBakedTx> {
    let mut messages = Vec::default();
    let bank_generator = BankMessageGenerator::<S>::create_invalid_transfer(admin);
    let bank_txns =
        bank_generator.create_default_encoded_txs_without_gas_usage::<IntegTestRuntime<S>>();
    messages.extend(bank_txns);
    messages
}

pub fn simulate_da_with_bad_sig(key: TestPrivateKey) -> Vec<FullyBakedTx> {
    let bank_generator: BankMessageGenerator<S> = BankMessageGenerator::with_minter(key.clone());
    let create_token_message = bank_generator.create_default_messages().remove(0);
    let tx = Transaction::<IntegTestRuntime<S>, S>::new_with_details(
        create_token_message.sender_key.pub_key(),
        <IntegTestRuntime<S> as EncodeCall<Bank<S>>>::to_decodable(create_token_message.content),
        // Use the signature of an empty message
        key.sign(&[]),
        create_token_message.nonce,
        create_token_message.details,
    );
    // Overwrite the signature with the signature of the empty message

    vec![encode_with_auth(tx)]
}

pub fn simulate_da_with_bad_nonce(key: TestPrivateKey) -> Vec<FullyBakedTx> {
    let bank_generator: BankMessageGenerator<S> = BankMessageGenerator::with_minter(key);
    let mut create_token_message = bank_generator.create_default_messages().remove(0);
    // Overwrite the nonce with the maximum value
    create_token_message.nonce = u64::MAX;
    let tx = create_token_message.to_tx::<IntegTestRuntime<S>>();
    vec![encode_with_auth(tx)]
}

pub fn simulate_da_with_bad_serialization(key: TestPrivateKey) -> Vec<FullyBakedTx> {
    let bank_generator: BankMessageGenerator<S> = BankMessageGenerator::with_minter(key);
    let create_token_message = bank_generator.create_default_messages().remove(0);
    let tx = Transaction::<IntegTestRuntime<S>, S>::new_signed_tx(
        &create_token_message.sender_key,
        &IntegTestRuntime::<S>::CHAIN_HASH,
        UnsignedTransaction::<IntegTestRuntime<S>, S>::new_with_details(
            <IntegTestRuntime<S> as EncodeCall<Bank<S>>>::to_decodable(
                create_token_message.content,
            ),
            create_token_message.nonce,
            create_token_message.details.clone(),
        ),
    );

    let mut serialized = encode_with_auth(tx);
    serialized.data[0] = serialized.data[0].wrapping_add(20);
    vec![serialized]
}

fn encode_with_auth(tx: Transaction<IntegTestRuntime<S>, S>) -> FullyBakedTx {
    let tx_bytes = RawTx::new(borsh::to_vec(&tx).unwrap());
    IntegTestRuntime::<S>::encode_with_standard_auth(tx_bytes)
}

pub fn simulate_da_with_incorrect_direct_registration_msg(admin: TestPrivateKey) -> RawTx {
    let bank_generator: BankMessageGenerator<S> = BankMessageGenerator::with_minter(admin);
    let create_token_message = bank_generator.create_default_messages().remove(0);
    let tx = create_token_message.to_tx::<IntegTestRuntime<S>>();

    RawTx {
        data: borsh::to_vec(&tx).unwrap(),
    }
}

pub fn simulate_da_with_multiple_direct_registration_msg(
    sequencers: Vec<Vec<u8>>,
    admin: TestPrivateKey,
) -> Vec<RawTx> {
    let mut messages = Vec::default();

    let sequencer_and_stake = sequencers
        .into_iter()
        .map(|address| (address, 100_000_000u64))
        .collect();
    let sequencer_registry_generator =
        SequencerRegistryMessageGenerator::<S>::generate_multiple_sequencer_registration(
            sequencer_and_stake,
            admin.clone(),
        );
    let default_messages = sequencer_registry_generator.create_default_messages();

    let nonce_offset = messages.len() as u64;
    for mut message in default_messages {
        message.nonce += nonce_offset;
        let tx = message.to_tx::<IntegTestRuntime<S>>();
        messages.push(RawTx {
            data: borsh::to_vec(&tx).unwrap(),
        });
    }

    messages
}
