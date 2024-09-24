use sov_modules_api::macros::config_gas_unit;
use sov_modules_api::{GasPrice, GasUnit};

pub const TEST_GAS: GasPrice<2> = config_gas_unit!("TEST_GAS_CONST_CORRECT");

fn main() {
    assert_eq!(TEST_GAS, GasPrice::<2>::from([1, 1]));
}
