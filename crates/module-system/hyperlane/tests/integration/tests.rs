use sov_hyperlane_integration::test_recipient::{
    self, CallMessage as RecipientCallMessage, Event, TestRecipient,
};
use sov_hyperlane_integration::{
    CallMessage, HyperlaneAddress, Ism, Mailbox as RawMailbox, MerkleTreeHooks, Message,
    MESSAGE_VERSION,
};
use sov_modules_api::capabilities::{
    authenticate, decode_sov_tx, AuthenticationError, AuthenticationOutput,
    BatchFromUnregisteredSequencer, FatalError, TransactionAuthenticator,
    UnregisteredAuthenticationError,
};
use sov_modules_api::macros::config_value;
use sov_modules_api::sov_universal_wallet::schema::SchemaGenerator;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{
    DispatchCall, HexHash, HexString, ProvableStateReader, Runtime, Spec, TxEffect,
};
use sov_state::User;
use sov_test_utils::runtime::genesis::zk::config::HighLevelZkGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_bare_runtime, AsUser, TestSpec, TestUser, TransactionTestCase};

type Mailbox<S> = RawMailbox<S, TestRecipient<S>>;

generate_bare_runtime! {
    name: TestRuntime,
    modules: [mailbox: Mailbox<S>, test_recipient: TestRecipient<S>, merkle_tree_hooks: MerkleTreeHooks<S>],
    operating_mode: sov_modules_api::runtime::OperatingMode::Zk,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::zk::config::MinimalZkGenesisConfig<S>,
    runtime_trait_impl_bounds: [S::Address: HyperlaneAddress],
    kernel_type: sov_test_utils::runtime::BasicKernel<'a, S>
}

/// The input for the runtime's authenticator functionality.
#[derive(std::fmt::Debug, Clone, borsh::BorshDeserialize, borsh::BorshSerialize)]
pub struct AuthenticatorInput(sov_modules_api::RawTx);

impl<S: Spec> TransactionAuthenticator<S> for TestRuntime<S>
where
    S::Address: HyperlaneAddress,
    Transaction<Self, S>: SchemaGenerator,
{
    type Decodable = <Self as DispatchCall>::Decodable;
    type Input = AuthenticatorInput;

    type Signature = sov_modules_api::transaction::TransactionWithoutCall<S>;

    fn decode_serialized_tx(
        &self,
        tx: &sov_modules_api::FullyBakedTx,
    ) -> Result<(Self::Decodable, Self::Signature), FatalError> {
        let tx: AuthenticatorInput = borsh::from_slice(&tx.data)
            .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;
        decode_sov_tx::<_, Self>(&tx.0.data)
    }

    fn authenticate<Accessor: ProvableStateReader<User, Spec = S>>(
        &self,
        tx: &sov_modules_api::FullyBakedTx,
        pre_exec_ws: &mut Accessor,
    ) -> Result<AuthenticationOutput<S, Self::Decodable>, AuthenticationError> {
        let input: AuthenticatorInput = borsh::from_slice(&tx.data).map_err(|e| {
            sov_modules_api::capabilities::fatal_deserialization_error::<_, S, _>(
                &tx.data,
                e,
                pre_exec_ws,
            )
        })?;

        ::sov_modules_api::capabilities::authenticate::<_, S, Self>(
            &input.0.data,
            &<Self as Runtime<S>>::CHAIN_HASH,
            pre_exec_ws,
        )
    }

    fn authenticate_unregistered<Accessor: ProvableStateReader<User, Spec = S>>(
        &self,
        batch: &BatchFromUnregisteredSequencer,
        pre_exec_ws: &mut Accessor,
    ) -> Result<AuthenticationOutput<S, Self::Decodable>, UnregisteredAuthenticationError> {
        authenticate::<_, S, Self>(
            &batch.tx.data,
            &<Self as Runtime<S>>::CHAIN_HASH,
            pre_exec_ws,
        )
        .map_err(|e| match e {
            AuthenticationError::FatalError(err, hash) => {
                UnregisteredAuthenticationError::FatalError(err, hash)
            }
            AuthenticationError::OutOfGas(err) => UnregisteredAuthenticationError::OutOfGas(err),
        })
    }

    fn add_standard_auth(tx: ::sov_modules_api::RawTx) -> Self::Input {
        AuthenticatorInput(tx)
    }
}

type S = TestSpec;
type RT = TestRuntime<S>;

#[allow(clippy::type_complexity)]
fn setup() -> (TestRunner<TestRuntime<S>, S>, TestUser<S>, TestUser<S>) {
    let genesis_config = HighLevelZkGenesisConfig::generate_with_additional_accounts(2);

    let admin_account = genesis_config.additional_accounts[0].clone();
    let extra_account = genesis_config.additional_accounts[1].clone();

    let genesis = GenesisConfig::from_minimal_config(genesis_config.clone().into(), (), (), ());

    (
        TestRunner::new_with_genesis(genesis.into_genesis_params(), Default::default()),
        admin_account,
        extra_account,
    )
}

#[test]
fn test_send_message_basic() {
    let (mut runner, admin, _) = setup();

    let recipient_address: HexHash = [5u8; 32].into();
    let message_body = b"Hello, world!";
    let message = Message {
        version: MESSAGE_VERSION,
        nonce: 0,
        origin_domain: test_recipient::MAGIC_SOV_CHAIN_DOMAIN, // Signal that the sender is a Sovereign SDK chain, so the sender address can be parsed. This makes the test output nicer.
        sender: admin.address().to_sender(), // The sender doesn't matter for this test
        dest_domain: config_value!("HYPERLANE_BRIDGE_DOMAIN"),
        recipient: recipient_address,
        body: message_body.to_vec().into(),
    };

    let admin_address = admin.address();

    runner.execute_transaction(TransactionTestCase {
        input: admin.create_plain_message::<RT, Mailbox<S>>(CallMessage::Process {
            message: HexString::new(message.encode().0.try_into().unwrap()),
            metadata: HexString::new(vec![].try_into().unwrap()),
        }),
        assert: Box::new(|result, _| {
            assert!(
                result.tx_receipt.is_reverted(),
                "No recipient is registered but the tx succeeded"
            );
        }),
    });

    register_recipient(&mut runner, &admin, recipient_address);
    runner.execute_transaction(TransactionTestCase {
        input: admin.create_plain_message::<RT, Mailbox<S>>(CallMessage::Process {
            message: HexString::new(message.encode().0.try_into().unwrap()),
            metadata: HexString::new(vec![].try_into().unwrap())
        }),
        assert: Box::new(move |result, _| {
            assert!(result.events.iter().any(|event| {
                matches!(
                    event,
                    TestRuntimeEvent::TestRecipient(Event::MessageReceived { sender, body, .. }) if *sender == admin_address && body == &HexString::new(message_body.to_vec()).to_string()
                )
            }));
        }),
    });
}

fn register_recipient(
    runner: &mut TestRunner<RT, S>,
    user: &TestUser<S>,
    recipient_address: HexHash,
) {
    register_recipient_with_ism(runner, user, recipient_address, Ism::AlwaysTrust);
}

fn register_recipient_with_ism(
    runner: &mut TestRunner<RT, S>,
    user: &TestUser<S>,
    recipient_address: HexHash,
    ism: Ism,
) {
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, TestRecipient<S>>(RecipientCallMessage::Register {
            address: recipient_address,
            ism,
        }),
        assert: Box::new(|result, _| {
            assert!(
                result.tx_receipt.is_successful(),
                "Recipient was not registered successfully"
            );
        }),
    });
}

/// Tests that messages are rejected if the destination domain is wrong (i.e. the message was intended for a different chain)
#[test]
fn test_send_message_to_wrong_domain() {
    let (mut runner, admin, _) = setup();

    let recipient_address: HexHash = [5u8; 32].into();
    let message_body = b"Hello, world!";
    let domain: u32 = config_value!("HYPERLANE_BRIDGE_DOMAIN");
    let message = Message {
        version: MESSAGE_VERSION,
        nonce: 0,
        origin_domain: test_recipient::MAGIC_SOV_CHAIN_DOMAIN, // Signal that the sender is a Sovereign SDK chain, so the sender address can be parsed.
        sender: admin.address().to_sender(), // The sender doesn't matter for this test
        dest_domain: domain.wrapping_add(1u32), // Modify the domain to be wrong
        recipient: recipient_address,
        body: message_body.to_vec().into(),
    };

    register_recipient(&mut runner, &admin, recipient_address);
    runner.execute_transaction(TransactionTestCase {
        input: admin.create_plain_message::<RT, Mailbox<S>>(CallMessage::Process {
            message: HexString::new(message.encode().0.try_into().unwrap()),
            metadata: HexString::new(vec![].try_into().unwrap())
        }),
        assert: Box::new(move |result, _| match result.tx_receipt {
            TxEffect::Reverted(reverted) => {
                assert!(reverted
                    .reason
                    .to_string()
                    .contains("Invalid message destination domain"),
                "Unexpected revert reason. Expected: Invalid message destination domain. Actual: {}",
                reverted.reason
            );
            }
            _ => {
                panic!("Unexpected tx receipt: {:?}", result.tx_receipt);
            }
        }),
    });
}

/// Tests that message cannot be replayed
#[test]
fn test_replay_message_delivery() {
    let (mut runner, admin, _) = setup();

    let recipient_address: HexHash = [5u8; 32].into();
    let message_body = b"Hello, world!";
    let message = Message {
        version: MESSAGE_VERSION,
        nonce: 0,
        origin_domain: test_recipient::MAGIC_SOV_CHAIN_DOMAIN, // Signal that the sender is a Sovereign SDK chain, so the sender address can be parsed.
        sender: admin.address().to_sender(), // The sender doesn't matter for this test
        dest_domain: config_value!("HYPERLANE_BRIDGE_DOMAIN"),
        recipient: recipient_address,
        body: message_body.to_vec().into(),
    };

    let admin_address = admin.address();
    register_recipient(&mut runner, &admin, recipient_address);
    runner.execute_transaction(TransactionTestCase {
        input: admin.create_plain_message::<RT, Mailbox<S>>(CallMessage::Process {
            message: HexString::new(message.encode().0.try_into().unwrap()),
            metadata: HexString::new(vec![].try_into().unwrap())
        }),
        assert: Box::new(move |result, _| {
            assert!(result.events.iter().any(|event| {
                matches!(
                    event,
                    TestRuntimeEvent::TestRecipient(Event::MessageReceived { sender, body, .. }) if *sender == admin_address && body == &HexString::new(message_body.to_vec()).to_string()
                )
            }));
        }),
    });
    runner.execute_transaction(TransactionTestCase {
        input: admin.create_plain_message::<RT, Mailbox<S>>(CallMessage::Process {
            message: HexString::new(message.encode().0.try_into().unwrap()),
            metadata: HexString::new(vec![].try_into().unwrap()),
        }),
        assert: Box::new(move |result, _| match result.tx_receipt {
            TxEffect::Reverted(reverted) => {
                assert!(
                    reverted.reason.to_string().contains("already processed"),
                    "Unexpected revert reason. Expected: Message _ already processed. Actual: {}",
                    reverted.reason
                );
            }
            _ => {
                panic!("Unexpected tx receipt: {:?}", result.tx_receipt);
            }
        }),
    });
}

/// Tests that messages are rejected by the "trusted relayer" ISM if the actual relayer is not the allowed relayer
#[test]
fn test_send_message_with_untrusted_relayer_to_trusted_relayer_ism() {
    let (mut runner, admin, test_user) = setup();

    let recipient_address: HexHash = [5u8; 32].into();
    let message_body = b"Hello, world!";
    let message = Message {
        version: MESSAGE_VERSION,
        nonce: 0,
        origin_domain: test_recipient::MAGIC_SOV_CHAIN_DOMAIN, // Signal that the sender is a Sovereign SDK chain, so the sender address can be parsed.
        sender: admin.address().to_sender(), // The sender doesn't matter for this test
        dest_domain: config_value!("HYPERLANE_BRIDGE_DOMAIN"),
        recipient: recipient_address,
        body: message_body.to_vec().into(),
    };
    let test_user_address = test_user.address();
    let admin_address = admin.address();

    register_recipient_with_ism(
        &mut runner,
        &admin,
        recipient_address,
        Ism::TrustedRelayer {
            relayer: test_user.address().to_sender(),
        },
    );
    // Check that the message is rejected by the "trusted relayer" ISM
    runner.execute_transaction(TransactionTestCase {
        input: admin.create_plain_message::<RT, Mailbox<S>>(CallMessage::Process {
            message: HexString::new(message.encode().0.try_into().unwrap()),
            metadata: HexString::new(vec![].try_into().unwrap()),
        }),
        assert: Box::new(move |result, _| match result.tx_receipt {
            TxEffect::Reverted(reverted) => {
                assert!(
                    reverted.reason.to_string().contains(&format!(
                        "Only {} is trusted",
                        test_user_address.to_sender(),
                    )),
                    "Unexpected revert reason. Expected: Only {} is trusted. Actual: {}",
                    test_user_address.to_sender(),
                    reverted.reason
                );
            }
            _ => {
                panic!("Unexpected tx receipt: {:?}", result.tx_receipt);
            }
        }),
    });

    // Now try again with the correct relayer. The message should be accepted
    runner.execute_transaction(TransactionTestCase {
        input: test_user.create_plain_message::<RT, Mailbox<S>>(CallMessage::Process {
            message: HexString::new(message.encode().0.try_into().unwrap()),
            metadata: HexString::new(vec![].try_into().unwrap())
        }),
        assert: Box::new(move |result, _|  {
            assert!(result.tx_receipt.is_successful(), "Message was not delivered successfully");
            assert!(result.events.iter().any(|event| {
                matches!(
                    event,
                    TestRuntimeEvent::TestRecipient(Event::MessageReceived { sender, body, .. }) if *sender == admin_address && body == &HexString::new(message_body.to_vec()).to_string()
                )
            }));
        }),
    });
}

/// Tests that messages are rejected if the version is wrong
#[test]
fn test_send_message_with_wrong_version() {
    let (mut runner, admin, _) = setup();

    let recipient_address: HexHash = [5u8; 32].into();
    let message_body = b"Hello, world!";
    let message = Message {
        version: 2, // Wrong version
        nonce: 0,
        origin_domain: test_recipient::MAGIC_SOV_CHAIN_DOMAIN, // Signal that the sender is a Sovereign SDK chain, so the sender address can be parsed.
        sender: admin.address().to_sender(), // The sender doesn't matter for this test
        dest_domain: config_value!("HYPERLANE_BRIDGE_DOMAIN"),
        recipient: recipient_address,
        body: message_body.to_vec().into(),
    };

    register_recipient(&mut runner, &admin, recipient_address);

    runner.execute_transaction(TransactionTestCase {
        input: admin.create_plain_message::<RT, Mailbox<S>>(CallMessage::Process {
            message: HexString::new(message.encode().0.try_into().unwrap()),
            metadata: HexString::new(vec![].try_into().unwrap()),
        }),
        assert: Box::new(move |result, _| match result.tx_receipt {
            TxEffect::Reverted(reverted) => {
                assert!(
                    reverted
                        .reason
                        .to_string()
                        .contains("Invalid message version"),
                    "Unexpected revert reason. Expected: Invalid message version. Actual: {}",
                    reverted.reason
                );
            }
            _ => {
                panic!("Unexpected tx receipt: {:?}", result.tx_receipt);
            }
        }),
    });
}
