use sov_modules_api::macros::config_gas_unit;
use sov_modules_api::GasUnit;

pub const TEST_GAS: GasUnit<1> = config_gas_unit!("TEST_GAS_CONST_CORRECT");

fn main() {
    assert_eq!(TEST_GAS, GasUnit::<1>::from([1]));
}
