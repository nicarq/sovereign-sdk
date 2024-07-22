use crate::transaction::TxGasMeter;
use crate::{GasArray, GasMeter, GasPrice, GasUnit};

#[test]
fn charge_gas_should_fail_if_not_enough_funds() {
    let gas_price = GasPrice::<2>::from_slice(&[1; 2]);

    let mut gas_meter = TxGasMeter::new(0, gas_price.clone());

    assert!(
        gas_meter
            .charge_gas(&GasUnit::<2>::from_slice(&[100; 2]))
            .is_err(),
        "The gas meter should not be able to charge gas if there is not enough funds"
    );
}

#[test]
fn refund_gas_should_fail_if_not_enough_funds_consumed() {
    let gas_price = GasPrice::<2>::from_slice(&[1; 2]);

    let mut gas_meter = TxGasMeter::new(100, gas_price.clone());

    assert!(
        gas_meter
            .refund_gas(&GasUnit::<2>::from_slice(&[100; 2]))
            .is_err(),
        "The gas meter should not be able to refund gas if there is not enough gas consumed"
    );
}

#[test]
fn try_charge_gas() {
    const REMAINING_FUNDS: u64 = 100;
    let gas_price = GasPrice::<2>::from_slice(&[1; 2]);

    let mut gas_meter = TxGasMeter::new(REMAINING_FUNDS, gas_price.clone());
    assert!(
        gas_meter
            .charge_gas(&GasUnit::<2>::from_slice(&[REMAINING_FUNDS / 2; 2]))
            .is_ok(),
        "It should be possible to charge gas"
    );
    assert_eq!(
        gas_meter.gas_used(),
        &GasUnit::from_slice(&[REMAINING_FUNDS / 2; 2]),
        "The gas used should be the same as the gas charged"
    );
    assert_eq!(gas_meter.gas_price(), &gas_price);
    assert_eq!(
        gas_meter.remaining_funds(),
        0,
        "There should be no more gas left in the meter"
    );

    assert!(
        gas_meter
            .charge_gas(&GasUnit::<2>::from_slice(&[1; 2]))
            .is_err(),
        "There should be no more gas left in the meter, hence charging more gas should fail"
    );
}

#[test]
fn try_refund_gas() {
    const REMAINING_FUNDS: u64 = 100;
    let gas_price = GasPrice::from_slice(&[1; 2]);

    let mut gas_meter = TxGasMeter::new(REMAINING_FUNDS, gas_price);
    assert!(
        gas_meter
            .charge_gas(&GasUnit::<2>::from_slice(&[REMAINING_FUNDS / 2; 2]))
            .is_ok(),
        "There should be enough gas left in the meter to charge"
    );
    assert_eq!(
        gas_meter.remaining_funds(),
        0,
        "There should be no more gas left in the meter"
    );

    assert!(
        gas_meter
            .refund_gas(&GasUnit::from_slice(&[REMAINING_FUNDS / 4; 2]))
            .is_ok(),
        "Enough gas should have been consumed to be refunded",
    );

    assert_eq!(
        gas_meter.gas_used(),
        &GasUnit::from_slice(&[REMAINING_FUNDS / 4; 2],),
        "The gas used amount should have decreased"
    );

    assert_eq!(
        gas_meter.remaining_funds(),
        REMAINING_FUNDS / 2,
        "Half of the gas should be refunded"
    );
}
