use sov_modules_api::macros::{config_gas_price, config_gas_unit};
use sov_modules_api::{GasPrice, GasUnit};

pub const TEST_GAS: GasUnit<2> = config_gas_unit!("TEST_GAS_CONST_CORRECT");
pub const TEST_GAS_2: GasPrice<4> = config_gas_price!("TEST_GAS_CONST_CORRECT_2");

fn main() {
    assert_eq!(TEST_GAS, GasUnit::<2>::from([1, 1]));
    assert_eq!(TEST_GAS_2, GasPrice::<4>::from([0, 1, 32, 18]));
}
