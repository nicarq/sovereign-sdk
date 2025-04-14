//! Test for the multisig ISM.

use std::str::FromStr;

use sov_bank::Amount;
use sov_hyperlane_integration::test_recipient::Event;
use sov_hyperlane_integration::{CallMessage, Ism, Message};
use sov_modules_api::{Address, BasicGasMeter, Context, GasPrice, GasUnit, HexString, TxEffect};
use sov_test_utils::{AsUser, TransactionTestCase};

use crate::runtime::{
    register_recipient_with_ism, setup, unlimited_gas_meter, Mailbox, TestRuntimeEvent, RT, S,
};

pub struct MultisigIsmTestData {
    pub message: Message,
    pub validators: Vec<HexString<[u8; 20]>>,
    pub signatures: Vec<Vec<u8>>,
}

const ORIGIN_DOMAIN: u32 = 1234u32;
const DESTINATION_DOMAIN: u32 = 4321u32;

fn get_message() -> Message {
    Message {
        version: 3,
        nonce: 69,
        origin_domain: ORIGIN_DOMAIN,
        sender: HexString::from_str(
            "0xafafafafafafafafafafafafafafafafafafafafafafafafafafafafafafafaf",
        )
        .unwrap(),
        dest_domain: DESTINATION_DOMAIN,
        recipient: HexString::from_str(
            "0xbebebebebebebebebebebebebebebebebebebebebebebebebebebebebebebebe",
        )
        .unwrap(),
        body: HexString(vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
    }
}

// Adapted from https://github.com/eigerco/hyperlane-monorepo/blob/b68fe264b3585ecd9d95a5ec2ec2d7defbe907d2/rust/sealevel/programs/ism/multisig-ism-message-id/src/processor.rs#L623
pub fn get_multisig_ism_test_data() -> MultisigIsmTestData {
    let message = get_message();

    // The hash being signed is equal to:
    // 0x3fd308215a20af20b137372f8a69fd336ebf93d57d4076a7c46e13f315255257

    // Validator 0:
    // Address: 0xE3DCDBbc248cE191bDc271f3FCcd0d95911BFC5D
    // Private Key: 0x788aa7213bd92ff92017d767fde0d75601425818c8e4b21e87314c2a4dcd6091
    let validator_0 = HexString::from_str("0xE3DCDBbc248cE191bDc271f3FCcd0d95911BFC5D").unwrap();
    // > await (new ethers.Wallet('0x788aa7213bd92ff92017d767fde0d75601425818c8e4b21e87314c2a4dcd6091')).signMessage(ethers.utils.arrayify('0x3fd308215a20af20b137372f8a69fd336ebf93d57d4076a7c46e13f315255257'))
    // '0x081d398e1452ae12267f63f224d3037b4bb3f496cb55c14a2076c5e27ed944ad6d8e10d3164bc13b5820846a3f19e013e1c551b67a3c863882f7b951acdab96d1c'
    let signature_0 = hex::decode("081d398e1452ae12267f63f224d3037b4bb3f496cb55c14a2076c5e27ed944ad6d8e10d3164bc13b5820846a3f19e013e1c551b67a3c863882f7b951acdab96d1c").unwrap();

    // Validator 1:
    // Address: 0xb25206874C24733F05CC0dD11924724A8E7175bd
    // Private Key: 0x4a599de3915f404d84a2ebe522bfe7032ebb1ca76a65b55d6eb212b129043a0e
    let validator_1 = HexString::from_str("0xb25206874C24733F05CC0dD11924724A8E7175bd").unwrap();
    // > await (new ethers.Wallet('0x4a599de3915f404d84a2ebe522bfe7032ebb1ca76a65b55d6eb212b129043a0e')).signMessage(ethers.utils.arrayify('0x3fd308215a20af20b137372f8a69fd336ebf93d57d4076a7c46e13f315255257'))
    // '0x0c189e25dea6bb93292af16fd0516f3adc8a19556714c0b8d624016175bebcba7a5fe8218dad6fc86faeb8104fad8390ccdec989d992e852553ea6b61fbb2eda1b'
    let signature_1 = hex::decode("0c189e25dea6bb93292af16fd0516f3adc8a19556714c0b8d624016175bebcba7a5fe8218dad6fc86faeb8104fad8390ccdec989d992e852553ea6b61fbb2eda1b").unwrap();

    // Validator 2:
    // Address: 0x28b8d0E2bBfeDe9071F8Ff3DaC9CcE3d3176DBd3
    // Private Key: 0x2cc76d56db9924ddc3388164454dfea9edd2d5f5da81102fd3594fc7c5281515
    let validator_2 = HexString::from_str("0x28b8d0E2bBfeDe9071F8Ff3DaC9CcE3d3176DBd3").unwrap();
    // > await (new ethers.Wallet('0x2cc76d56db9924ddc3388164454dfea9edd2d5f5da81102fd3594fc7c5281515')).signMessage(ethers.utils.arrayify('0x3fd308215a20af20b137372f8a69fd336ebf93d57d4076a7c46e13f315255257'))
    // '0x5493449e8a09c1105195ecf913997de51bd50926a075ad98fe3e845e0a11126b5212a2cd1afdd35a44322146d31f8fa3d179d8a9822637d8db0e2fa8b3d292421b'
    let signature_2 = hex::decode("5493449e8a09c1105195ecf913997de51bd50926a075ad98fe3e845e0a11126b5212a2cd1afdd35a44322146d31f8fa3d179d8a9822637d8db0e2fa8b3d292421b").unwrap();

    MultisigIsmTestData {
        message,
        // checkpoint,
        validators: vec![validator_0, validator_1, validator_2],
        signatures: vec![signature_0, signature_1, signature_2],
    }
}

fn dummy_context() -> Context<S> {
    Context::new(
        Address::new([0; Address::LENGTH]),
        Default::default(),
        Address::new([0; Address::LENGTH]),
        [1; 32].into(),
    )
}

// Encoded metadata taken from: https://github.com/eigerco/hyperlane-monorepo/blob/b68fe264b3585ecd9d95a5ec2ec2d7defbe907d2/rust/sealevel/programs/ism/multisig-ism-message-id/src/processor.rs#L623
const METADATA_WITHOUT_SIGNATURES: &str = "0xababababababababababababababababababababababababababababababababcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd00000046";

#[test]
fn test_verify_valid() {
    let data = get_multisig_ism_test_data();
    let mut metadata: HexString = HexString::from_str(METADATA_WITHOUT_SIGNATURES).unwrap();
    // Put two valid signatures in the metadata
    metadata.0.extend(data.signatures[0].iter());
    metadata.0.extend(data.signatures[1].iter());
    let ism = Ism::MessageIdMultisig {
        validators: data.validators.try_into().unwrap(),
        threshold: 2,
    };
    assert!(ism
        .verify(
            &dummy_context(),
            &data.message,
            &metadata,
            &mut unlimited_gas_meter()
        )
        .is_ok());
}

#[test]
fn test_verify_wrong_message() {
    let mut data = get_multisig_ism_test_data();
    let mut metadata: HexString = HexString::from_str(METADATA_WITHOUT_SIGNATURES).unwrap();
    // Put two valid signatures in the metadata
    metadata.0.extend(data.signatures[0].iter());
    metadata.0.extend(data.signatures[1].iter());
    let ism = Ism::MessageIdMultisig {
        validators: data.validators.try_into().unwrap(),
        threshold: 2,
    };
    // Tweak the message so that the signatures no longer fit it.
    data.message.nonce = u32::MAX;
    assert!(ism
        .verify(
            &dummy_context(),
            &data.message,
            &metadata,
            &mut unlimited_gas_meter()
        )
        .is_err());
}

#[test]
fn test_verify_duplicate_signatures() {
    let data: MultisigIsmTestData = get_multisig_ism_test_data();
    let mut metadata: HexString = HexString::from_str(METADATA_WITHOUT_SIGNATURES).unwrap();
    // Put the same signature twice
    metadata.0.extend(data.signatures[0].iter());
    metadata.0.extend(data.signatures[0].iter());
    let ism = Ism::MessageIdMultisig {
        validators: data.validators.try_into().unwrap(),
        threshold: 2,
    };
    assert!(ism
        .verify(
            &dummy_context(),
            &data.message,
            &metadata,
            &mut unlimited_gas_meter()
        )
        .is_err());
}

#[test]
fn test_verify_not_enough_signatures() {
    let data: MultisigIsmTestData = get_multisig_ism_test_data();
    let mut metadata: HexString = HexString::from_str(METADATA_WITHOUT_SIGNATURES).unwrap();
    // Only put one signature in the metadata
    metadata.0.extend(data.signatures[0].iter());
    let ism = Ism::MessageIdMultisig {
        validators: data.validators.try_into().unwrap(),
        threshold: 2,
    };
    assert!(ism
        .verify(
            &dummy_context(),
            &data.message,
            &metadata,
            &mut unlimited_gas_meter()
        )
        .is_err());
}

#[test]
fn test_verify_invalid_signatures() {
    let mut data: MultisigIsmTestData = get_multisig_ism_test_data();
    let mut metadata: HexString = HexString::from_str(METADATA_WITHOUT_SIGNATURES).unwrap();
    // Modify the first signature so that it's invalid
    data.signatures[0][0] += 1;
    metadata.0.extend(data.signatures[0].iter());
    metadata.0.extend(data.signatures[1].iter());
    metadata.0[64] = 27;
    let ism = Ism::MessageIdMultisig {
        validators: data.validators.try_into().unwrap(),
        threshold: 2,
    };
    assert!(ism
        .verify(
            &dummy_context(),
            &data.message,
            &metadata,
            &mut unlimited_gas_meter()
        )
        .is_err());
}

#[test]
fn test_verify_too_many_signatures() {
    let data: MultisigIsmTestData = get_multisig_ism_test_data();
    let mut metadata: HexString = HexString::from_str(METADATA_WITHOUT_SIGNATURES).unwrap();
    // Put three signatures in the metadata
    metadata.0.extend(data.signatures[0].iter());
    metadata.0.extend(data.signatures[1].iter());
    metadata.0.extend(data.signatures[2].iter());
    let ism = Ism::MessageIdMultisig {
        validators: data.validators.try_into().unwrap(),
        threshold: 2,
    };
    assert!(ism
        .verify(
            &dummy_context(),
            &data.message,
            &metadata,
            &mut unlimited_gas_meter()
        )
        .is_err());
}

#[test]
fn test_verify_out_of_order_signatures() {
    let data: MultisigIsmTestData = get_multisig_ism_test_data();
    let mut metadata: HexString = HexString::from_str(METADATA_WITHOUT_SIGNATURES).unwrap();

    // Put the last two signatures in the metadata
    metadata.0.extend(data.signatures[2].iter());
    metadata.0.extend(data.signatures[1].iter());
    let ism = Ism::MessageIdMultisig {
        validators: data.validators.try_into().unwrap(),
        threshold: 2,
    };
    assert!(ism
        .verify(
            &dummy_context(),
            &data.message,
            &metadata,
            &mut unlimited_gas_meter()
        )
        .is_ok());
}

#[test]
fn test_verify_not_enough_gas() {
    let data: MultisigIsmTestData = get_multisig_ism_test_data();
    let mut metadata: HexString = HexString::from_str(METADATA_WITHOUT_SIGNATURES).unwrap();

    // Put the last two signatures in the metadata
    metadata.0.extend(data.signatures[2].iter());
    metadata.0.extend(data.signatures[1].iter());
    let ism = Ism::MessageIdMultisig {
        validators: data.validators.try_into().unwrap(),
        threshold: 2,
    };
    let mut gas_meter = BasicGasMeter::new_with_funds_and_gas(
        Amount(100),
        GasUnit::from([1, 1]),
        GasPrice::from([Amount(100), Amount(100)]),
    );
    assert!(ism
        .verify(&dummy_context(), &data.message, &metadata, &mut gas_meter)
        .is_err());
}

/// Run an end-to-end test that verifies a valid message is processed correctly with the multisig ISM
#[test]
fn test_verify_valid_end_to_end() {
    std::env::set_var(
        "SOV_TEST_CONST_OVERRIDE_HYPERLANE_BRIDGE_DOMAIN",
        DESTINATION_DOMAIN.to_string(),
    );

    let data: MultisigIsmTestData = get_multisig_ism_test_data();
    let mut metadata: HexString = HexString::from_str(METADATA_WITHOUT_SIGNATURES).unwrap();
    // Put two valid signatures in the metadata
    metadata.0.extend(data.signatures[0].iter());
    metadata.0.extend(data.signatures[1].iter());
    let ism = Ism::MessageIdMultisig {
        validators: data.validators.try_into().unwrap(),
        threshold: 2,
    };

    let (mut runner, admin, _) = setup();

    register_recipient_with_ism(
        &mut runner,
        &admin,
        HexString::from_str("0xbebebebebebebebebebebebebebebebebebebebebebebebebebebebebebebebe")
            .unwrap(),
        ism,
    );

    let expected_sender =
        HexString::from_str("0xafafafafafafafafafafafafafafafafafafafafafafafafafafafafafafafaf")
            .unwrap();

    runner.execute_transaction(TransactionTestCase {
        input: admin.create_plain_message::<RT, Mailbox<S>>(CallMessage::Process {
            message: HexString(get_message().encode().0.try_into().unwrap()),
            metadata: HexString(metadata.0.try_into().unwrap()),
        }),
        assert: Box::new(move |result, _| {
            assert!(result.events.iter().any(|event| {
                matches!(
                    event,
                    TestRuntimeEvent::TestRecipient(Event::MessageReceivedGeneric { sender, body, .. }) if *sender == expected_sender && body == &HexString::new(vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
                )
            }),
            "Did not receive expected message. {:?}, tx receipt: {:?}",
            result.events,
            result.tx_receipt,
        );
        }),
    });
}

/// Run an end-to-end test that verifies an invalid message is rejected with the multisig ISM
#[test]
fn test_verify_invalid_end_to_end() {
    std::env::set_var(
        "SOV_TEST_CONST_OVERRIDE_HYPERLANE_BRIDGE_DOMAIN",
        DESTINATION_DOMAIN.to_string(),
    );

    let data: MultisigIsmTestData = get_multisig_ism_test_data();
    let mut metadata: HexString = HexString::from_str(METADATA_WITHOUT_SIGNATURES).unwrap();
    // Duplicate the first signature
    metadata.0.extend(data.signatures[0].iter());
    metadata.0.extend(data.signatures[0].iter());
    let ism = Ism::MessageIdMultisig {
        validators: data.validators.try_into().unwrap(),
        threshold: 2,
    };

    let (mut runner, admin, _) = setup();

    register_recipient_with_ism(
        &mut runner,
        &admin,
        HexString::from_str("0xbebebebebebebebebebebebebebebebebebebebebebebebebebebebebebebebe")
            .unwrap(),
        ism,
    );

    runner.execute_transaction(TransactionTestCase {
        input: admin.create_plain_message::<RT, Mailbox<S>>(CallMessage::Process {
            message: HexString(get_message().encode().0.try_into().unwrap()),
            metadata: HexString(metadata.0.try_into().unwrap()),
        }),
        assert: Box::new(move |result, _| match result.tx_receipt {
            TxEffect::Reverted(reverted) => {
                assert!(reverted.reason.to_string().contains("Not enough unique validators signed"), "Unexpected revert reason. Expected: Not enough unique validators signed the message. Actual: {}", reverted.reason);
            }
            _ => {
                panic!("Unexpected tx receipt: {:?}", result.tx_receipt);
            }
        }),
    });
}
