use sov_modules_api::macros::{config_gas_unit, config_value};
use sov_modules_api::GasUnit;

const GAS_DIMENSIONS: usize = config_value!("GAS_DIMENSIONS");

pub const TEST_GAS: GasUnit<GAS_DIMENSIONS> = config_gas_unit!("TEST_TOKEN_ID");

fn main() {
    assert_eq!(TEST_GAS, GasUnit::<GAS_DIMENSIONS>::from([1, 1]));
}
