use sov_bank::TokenId;
use sov_modules_api::macros::config_bech32_constant;


#[config_bech32_constant]
const TEST_ARRAY_OF_U8: TokenId;


fn main() {
    assert_eq!(TEST_ARRAY_OF_U8, TokenId::from([0; 32]));
}
