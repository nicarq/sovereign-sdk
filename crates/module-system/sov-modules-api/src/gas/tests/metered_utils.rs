use borsh::{BorshDeserialize, BorshSerialize};
use sha2::Sha256;
use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
use sov_mock_zkvm::crypto::Ed25519Signature;
use sov_mock_zkvm::MockZkVerifier;
use sov_rollup_interface::crypto::PrivateKey;
use sov_rollup_interface::execution_mode::Native;
use sov_test_utils::storage::new_finalized_storage;

use crate::default_spec::DefaultSpec;
use crate::gas::GasArray;
use crate::{
    Gas, GasPrice, GasUnit, MeteredBorshDeserialize, MeteredBorshDeserializeError, MeteredHasher,
    MeteredSigVerificationError, MeteredSignature, Spec, WorkingSet,
};
type S = DefaultSpec<MockZkVerifier, MockZkVerifier, Native>;

fn create_working_set(
    remaining_funds: u64,
    gas_price: &<<S as Spec>::Gas as Gas>::Price,
) -> WorkingSet<S> {
    let tmpdir = tempfile::tempdir().unwrap();
    let storage = new_finalized_storage(tmpdir.path());
    WorkingSet::new_with_gas_meter(storage, remaining_funds, gas_price)
}

#[test]
fn test_metered_hasher_happy_path() {
    let gas_to_charge_for_hash_update = GasUnit::<2>::from_slice(&[5, 5]);
    let gas_to_charge_for_hash_finalize = GasUnit::<2>::from_slice(&[2, 2]);

    let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

    let data = [1_u8; 32];

    let remaining_funds = gas_to_charge_for_hash_update
        .clone()
        .scalar_product(data.len() as u64)
        .value(&gas_price)
        + gas_to_charge_for_hash_finalize.value(&gas_price);

    let mut ws = create_working_set(remaining_funds, &gas_price);

    let mut hasher = MeteredHasher::<_, _, Sha256>::new_with_custom_price(
        &mut ws,
        gas_to_charge_for_hash_update,
        gas_to_charge_for_hash_finalize,
    );

    assert!(
        hasher.update(&data).is_ok(),
        "Hasher should be able to update"
    );
    assert!(
        hasher.finalize().is_ok(),
        "Hasher should be able to finalize"
    );
}

#[test]
fn test_metered_hasher_not_enough_gas_to_finalize() {
    let gas_to_charge_for_hash_update = GasUnit::<2>::from_slice(&[5, 5]);
    let gas_to_charge_for_hash_finalize = GasUnit::<2>::from_slice(&[2, 2]);

    let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

    let data = [1_u8; 32];

    let remaining_funds = gas_to_charge_for_hash_update
        .clone()
        .scalar_product(data.len() as u64)
        .value(&gas_price);

    let mut ws = create_working_set(remaining_funds, &gas_price);

    let mut hasher = MeteredHasher::<_, _, Sha256>::new_with_custom_price(
        &mut ws,
        gas_to_charge_for_hash_update,
        gas_to_charge_for_hash_finalize,
    );

    assert!(
        hasher.update(&data).is_ok(),
        "Hasher should be able to update"
    );
    assert!(
        hasher.finalize().is_err(),
        "Hasher should not be able to finalize because it should not have enough gas"
    );
}

#[test]
fn test_metered_hasher_not_enough_gas_to_update() {
    let gas_to_charge_for_hash_update = GasUnit::<2>::from_slice(&[5, 5]);
    let gas_to_charge_for_hash_finalize = GasUnit::<2>::from_slice(&[2, 2]);

    let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

    let data = [1_u8; 32];

    let remaining_funds = gas_to_charge_for_hash_update
        .clone()
        .scalar_product(data.len() as u64 - 1)
        .value(&gas_price);

    let mut ws = create_working_set(remaining_funds, &gas_price);

    let mut hasher = MeteredHasher::<_, _, Sha256>::new_with_custom_price(
        &mut ws,
        gas_to_charge_for_hash_update,
        gas_to_charge_for_hash_finalize,
    );

    assert!(
        hasher.update(&data).is_err(),
        "Hasher should be not able to update because it should not have enough gas"
    );
}

#[test]
fn test_metered_signature() {
    let gas_to_charge_for_signature = GasUnit::<2>::from_slice(&[5, 5]);
    let mut fixed_cost = GasUnit::<2>::from_slice(&[1000, 1000]);

    let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

    let data = [1_u8; 32];

    let ed25519 = Ed25519PrivateKey::generate();
    let signature = ed25519.sign(&data);

    let metered_signature = MeteredSignature::<_, Ed25519Signature>::new_with_price(
        signature,
        fixed_cost.clone(),
        gas_to_charge_for_signature.clone(),
    );

    let remaining_funds = fixed_cost
        .combine(
            gas_to_charge_for_signature
                .clone()
                .scalar_product(data.len() as u64),
        )
        .value(&gas_price);

    let mut ws = create_working_set(remaining_funds, &gas_price);

    assert!(
            metered_signature
                .verify(&ed25519.pub_key(), &data, &mut ws)
                .is_ok(),
            "Signature should be valid and there should be enough gas available in the metered working set"
        );
}

#[test]
fn test_metered_signature_not_enough_gas() {
    let gas_to_charge_for_signature = GasUnit::<2>::from_slice(&[5, 5]);
    let mut fixed_cost = GasUnit::<2>::from_slice(&[1000, 1000]);

    let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

    let data = [1_u8; 32];

    let ed25519 = Ed25519PrivateKey::generate();
    let signature = ed25519.sign(&data);

    let metered_signature = MeteredSignature::<_, Ed25519Signature>::new_with_price(
        signature,
        fixed_cost.clone(),
        gas_to_charge_for_signature.clone(),
    );

    let remaining_funds = fixed_cost
        .combine(
            gas_to_charge_for_signature
                .clone()
                .scalar_product(data.len() as u64 - 1),
        )
        .value(&gas_price);

    let mut ws = create_working_set(remaining_funds, &gas_price);

    assert!(
        matches!(
            metered_signature.verify(&ed25519.pub_key(), &data, &mut ws),
            Err(MeteredSigVerificationError::GasError(..))
        ),
        "There should not be enough gas available in the metered working set"
    );
}

#[derive(Debug, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
pub struct BorshTestStruct {
    pub field1: u32,
    pub field2: u32,
}

#[test]
fn test_metered_deserializer() {
    let gas_to_charge_for_deserialization = GasUnit::<2>::from_slice(&[5, 5]);
    let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

    let data = BorshTestStruct {
        field1: 1,
        field2: 2,
    };
    let serialized_data = borsh::to_vec(&data).unwrap();

    let remaining_funds = gas_to_charge_for_deserialization
        .clone()
        .scalar_product(serialized_data.len() as u64)
        .value(&gas_price);

    let mut ws = create_working_set(remaining_funds, &gas_price);

    let deserialized_data =
            <BorshTestStruct as MeteredBorshDeserialize::<GasUnit<2>>>::deserialize_with_custom_cost(
                &mut serialized_data.as_slice(),
                &mut ws,
                gas_to_charge_for_deserialization,
            )
            .expect("Deserialization should succeed because there should be enough gas available in the gas meter");

    assert_eq!(
        deserialized_data, data,
        "The deserialized data should match the original data"
    );
}

#[test]
fn test_metered_deserializer_not_enough_gas() {
    let gas_to_charge_for_deserialization = GasUnit::<2>::from_slice(&[5, 5]);
    let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

    let data = BorshTestStruct {
        field1: 1,
        field2: 2,
    };
    let serialized_data = borsh::to_vec(&data).unwrap();

    let remaining_funds = gas_to_charge_for_deserialization
        .clone()
        .scalar_product(serialized_data.len() as u64 - 1)
        .value(&gas_price);

    let mut ws = create_working_set(remaining_funds, &gas_price);

    let deserialized_err =
            <BorshTestStruct as MeteredBorshDeserialize::<GasUnit<2>>>::deserialize_with_custom_cost(
                &mut serialized_data.as_slice(),
                &mut ws,
                gas_to_charge_for_deserialization,
            )
            .expect_err("Deserialization should fail because there should not be enough gas available in the gas meter");

    assert!(
        matches!(deserialized_err, MeteredBorshDeserializeError::GasError(..)),
        "The deserialized error should be a gas error"
    );
}

#[test]
fn test_metered_deserializer_invalid_data() {
    let gas_to_charge_for_deserialization = GasUnit::<2>::from_slice(&[5, 5]);
    let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

    let data = BorshTestStruct {
        field1: 1,
        field2: 2,
    };
    let serialized_data = borsh::to_vec(&data).unwrap();

    let remaining_funds = gas_to_charge_for_deserialization
        .clone()
        .scalar_product(serialized_data.len() as u64)
        .value(&gas_price);

    let mut ws = create_working_set(remaining_funds, &gas_price);

    let deserialize_err =
        <BorshTestStruct as MeteredBorshDeserialize<GasUnit<2>>>::deserialize_with_custom_cost(
            &mut &serialized_data[1..],
            &mut ws,
            gas_to_charge_for_deserialization,
        )
        .expect_err("Deserialization should fail because the data is invalid");

    assert!(
        matches!(deserialize_err, MeteredBorshDeserializeError::IOError(..)),
        "The deserialized error should be a borsh deserialize error"
    );
}
