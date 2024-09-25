use sov_modules_api::macros::config_gas_unit;
use sov_modules_api::GasUnit;

pub const TEST_GAS: GasUnit<1> = config_gas_unit!("TEST_GAS_CONST_INCORRECT_DIM");

fn main() {}
