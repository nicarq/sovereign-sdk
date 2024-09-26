use crate::{GasMeter, GasPrice, GasUnit, UnlimitedGasMeter};

#[test]
fn charge_gas_should_always_succeed() {
    let gas_price = GasPrice::<2>::from([1; 2]);

    let mut gas_meter = UnlimitedGasMeter::new_with_price(gas_price.clone());

    assert!(
        gas_meter
            .charge_gas(&GasUnit::<2>::from([u64::MAX; 2]))
            .is_ok(),
        "The unlimited gas meter should never run out of gas"
    );
}

#[test]
fn refund_gas_should_fail_if_not_enough_funds_consumed() {
    let gas_price = GasPrice::<2>::from([1; 2]);

    let mut gas_meter = UnlimitedGasMeter::new_with_price(gas_price.clone());

    assert!(
        gas_meter.refund_gas(&GasUnit::<2>::from([100; 2])).is_err(),
        "The gas meter should not be able to refund gas if there is not enough gas consumed"
    );
}

#[test]
fn try_charge_gas() {
    const REMAINING_FUNDS: u64 = 100;
    let gas_price = GasPrice::<2>::from([1; 2]);

    let mut gas_meter = UnlimitedGasMeter::new_with_price(gas_price.clone());
    assert!(
        gas_meter
            .charge_gas(&GasUnit::<2>::from([REMAINING_FUNDS / 2; 2]))
            .is_ok(),
        "It should be possible to charge gas"
    );
    assert_eq!(
        gas_meter.gas_used(),
        &GasUnit::from([REMAINING_FUNDS / 2; 2]),
        "The gas used should be the same as the gas charged"
    );
    assert_eq!(gas_meter.gas_price(), &gas_price);

    assert!(
        gas_meter.charge_gas(&GasUnit::<2>::from([1; 2])).is_ok(),
        "The unlimited gas meter should never run out of gas"
    );
}

#[test]
fn try_refund_gas() {
    const FUNDS_TO_CONSUME: u64 = 100;
    let gas_price = GasPrice::from([1; 2]);

    let mut gas_meter = UnlimitedGasMeter::new_with_price(gas_price);
    assert!(
        gas_meter
            .charge_gas(&GasUnit::<2>::from([FUNDS_TO_CONSUME / 2; 2]))
            .is_ok(),
        "There should be enough gas left in the meter to charge"
    );

    assert!(
        gas_meter
            .refund_gas(&GasUnit::from([FUNDS_TO_CONSUME / 4; 2]))
            .is_ok(),
        "Enough gas should have been consumed to be refunded",
    );

    assert_eq!(
        gas_meter.gas_used(),
        &GasUnit::from([FUNDS_TO_CONSUME / 4; 2],),
        "The gas used amount should have decreased"
    );
}

#[test]
fn test_gas_display_multidimensional() {
    let gas_unit = GasUnit::<2>::from([100, 50]);
    assert_eq!(
        "GasUnit[100, 50]",
        gas_unit.to_string(),
        "The gas unit should be displayed correctly"
    );

    let gas_price = GasPrice::<2>::from([100, 50]);
    assert_eq!(
        "GasPrice[100, 50]",
        gas_price.to_string(),
        "The gas unit should be displayed correctly"
    );
}
