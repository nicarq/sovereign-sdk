use sov_mock_zkvm::MockZkvm;
use sov_modules_macros::config_value;
use sov_rollup_interface::execution_mode::Native;
use sov_state::{SlotKey, SlotValue, User};
use sov_test_utils::storage::SimpleStorageManager;
use sov_test_utils::MockDaSpec;

use super::traits::{StateReader, StateWriter};
use crate::default_spec::DefaultSpec;
use crate::{Gas, GasArray, GetGasInfo, Spec, WorkingSet};

type S = DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, Native>;

fn create_working_set(
    remaining_funds: u64,
    gas_price: &<<S as Spec>::Gas as Gas>::Price,
) -> WorkingSet<S> {
    let storage_manager = SimpleStorageManager::new();
    let storage = storage_manager.create_storage();
    WorkingSet::new_with_gas_meter(storage, remaining_funds, gas_price)
}

#[test]
fn test_charge_gas_to_set() {
    let gas_price = <<S as Spec>::Gas as Gas>::Price::from([1; 2]);
    let value = SlotValue::from("value");
    let gas_set_cost = <S as Spec>::Gas::from(config_value!("GAS_TO_CHARGE_PER_BYTE_FOR_WRITE"))
        .checked_scalar_product(value.size())
        .unwrap();
    let remaining_funds = gas_set_cost.value(&gas_price);

    let mut working_set = create_working_set(remaining_funds, &gas_price);

    assert!(
       StateWriter::<User>::set(&mut working_set, &SlotKey::from_slice(b"key"), SlotValue::from("value"))
            .is_ok(),
        "The set operation should succeed because there should be enough funds in the metered working set"
    );

    assert!(
       StateWriter::<User>::set(&mut working_set, &SlotKey::from_slice(b"key"), SlotValue::from("value2"))
            .is_err(),
        "The set operation should fail because there should be not enough funds left in the metered working set"
    );
}

#[test]
fn test_charge_gas_retrieve() {
    let gas_price = <<S as Spec>::Gas as Gas>::Price::from([1; 2]);
    let gas_access_cost = <S as Spec>::Gas::from(config_value!("GAS_TO_CHARGE_FOR_ACCESS"));
    let remaining_funds = gas_access_cost.value(&gas_price);

    let mut working_set = create_working_set(remaining_funds, &gas_price);

    assert!(
        StateReader::<User>::get(&mut working_set, &SlotKey::from_slice(b"key")) 
            .is_ok(),
        "The get operation should succeed because there should be enough funds in the metered working set"
    );

    assert!(
        StateReader::<User>::get(&mut working_set, &SlotKey::from_slice(b"key2")) 
            .is_err(),
        "The get operation should fail because there should be not enough funds left in the metered working set"
    );
}

#[test]
fn test_charge_gas_set_then_retrieve() {
    let gas_price = <<S as Spec>::Gas as Gas>::Price::from([1; 2]);

    let gas_access_cost = <S as Spec>::Gas::from(config_value!("GAS_TO_CHARGE_FOR_ACCESS"));
    let gas_hot_access_refund =
        <S as Spec>::Gas::from(config_value!("GAS_TO_REFUND_FOR_HOT_ACCESS"));

    let value = SlotValue::from("value");
    let gas_set_cost = <S as Spec>::Gas::from(config_value!("GAS_TO_CHARGE_PER_BYTE_FOR_WRITE"))
        .checked_scalar_product(value.size())
        .unwrap();

    let remaining_funds = gas_access_cost.value(&gas_price) + gas_set_cost.value(&gas_price);

    let mut working_set = create_working_set(remaining_funds, &gas_price);

    assert!(
        StateWriter::<User>::set(&mut working_set, &SlotKey::from_slice(b"key"), value)
            .is_ok(),
        "The set operation should succeed because there should be enough funds in the metered working set"
    );

    assert_eq!(
        working_set.gas_info().remaining_funds,
        gas_access_cost.value(&gas_price),
        "The remaining funds should have decreased by the amount of gas to charge for a write"
    );

    match StateReader::<User>::get(&mut working_set, &SlotKey::from_slice(b"key")){
        Ok(value) => {
            assert_eq!(value, Some(SlotValue::from("value")), "The value read should be equal to the value previously written");
        }
        Err(err) => panic!("The get operation should succeed because there should be enough funds in the metered working set, error {err:?}"),
    }

    // There should be some funds left in the metered working set because the second operation was a hot read
    let expected_remaining_funds = gas_hot_access_refund.value(&gas_price);
    assert_eq!(
        working_set.gas_info().remaining_funds,
        expected_remaining_funds,
        "The remaining funds should be equal to the expected value, some gas should have been refunded because of the hot read"
    );
}
