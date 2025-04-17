use secp256k1::rand::rngs::OsRng;
use secp256k1::{Message, Secp256k1, SecretKey};
use sov_bank::Amount;
use sov_hyperlane_integration::crypto::eth_address_from_public_key;
use sov_hyperlane_integration::test_recipient::{
    CallMessage as RecipientCallMessage, TestRecipient,
};
use sov_hyperlane_integration::{
    EthAddress, HyperlaneAddress, Ism, Mailbox as RawMailbox, MerkleTreeHook, ValidatorSignature,
};
use sov_modules_api::gas::GasArray;
use sov_modules_api::{BasicGasMeter, Gas, HexHash, HexString, Spec};
use sov_test_utils::runtime::genesis::zk::config::HighLevelZkGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_runtime, AsUser, TestSpec, TestUser, TransactionTestCase};

pub type Mailbox<S> = RawMailbox<S, TestRecipient<S>>;
pub type S = TestSpec;
pub type RT = TestRuntime<S>;

generate_runtime! {
    name: TestRuntime,
    modules: [mailbox: Mailbox<S>, test_recipient: TestRecipient<S>, merkle_tree_hook: MerkleTreeHook<S>],
    operating_mode: sov_modules_api::runtime::OperatingMode::Zk,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::zk::config::MinimalZkGenesisConfig<S>,
    runtime_trait_impl_bounds: [S::Address: HyperlaneAddress],
    kernel_type: sov_test_utils::runtime::BasicKernel<'a, S>,
    auth_type: sov_modules_api::capabilities::RollupAuthenticator<S, TestRuntime<S>>,
    auth_call_wrapper: |call| call,
}

#[allow(clippy::type_complexity)]
pub fn setup() -> (TestRunner<TestRuntime<S>, S>, TestUser<S>, TestUser<S>) {
    let genesis_config = HighLevelZkGenesisConfig::generate_with_additional_accounts(2);

    let admin_account = genesis_config.additional_accounts[0].clone();
    let extra_account = genesis_config.additional_accounts[1].clone();

    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), (), (), ());

    (
        TestRunner::new_with_genesis(genesis.into_genesis_params(), Default::default()),
        admin_account,
        extra_account,
    )
}

pub fn register_recipient(
    runner: &mut TestRunner<RT, S>,
    user: &TestUser<S>,
    recipient_address: HexHash,
) {
    register_recipient_with_ism(runner, user, recipient_address, Ism::AlwaysTrust);
}

pub fn register_recipient_with_ism(
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

pub fn random_validator() -> (SecretKey, EthAddress) {
    let secp = Secp256k1::new();
    let (secret_key, public_key) = secp.generate_keypair(&mut OsRng);
    let address = eth_address_from_public_key(public_key, &mut unlimited_gas_meter()).unwrap();
    (secret_key, address)
}

pub fn sign(digest: [u8; 32], sk: &SecretKey) -> ValidatorSignature {
    let secp = Secp256k1::new();
    let signature = secp.sign_ecdsa_recoverable(&Message::from_digest(digest), sk);
    let (recovery_id, sig_bytes) = signature.serialize_compact();

    let mut bytes = [0u8; 65];
    bytes[..64].copy_from_slice(&sig_bytes);
    bytes[64] = recovery_id.to_i32() as u8;
    HexString(bytes)
}

pub fn unlimited_gas_meter() -> BasicGasMeter<S> {
    BasicGasMeter::new_with_funds_and_gas(
        Amount::MAX,
        <<S as Spec>::Gas as Gas>::max(),
        <<S as Spec>::Gas as Gas>::Price::ZEROED,
    )
}
