use sov_modules_api::macros::config_bech32_constant;
use sov_modules_api::impl_hash32_type;


impl_hash32_type!(MyTokenId, MyTokenBech, "token_with_long_prefix");

#[config_bech32_constant]
const TEST_TOKEN_ID: MyTokenId;


fn main() {
    assert_eq!(TEST_TOKEN_ID, MyTokenId::from([0; 32]));
}
