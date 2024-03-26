use sov_bank::TokenId;
use sov_modules_api::macros::config_bech32_constant;
use sov_modules_api::macros::config_constant;
#[config_constant]
pub const TEST_U32: u32;

#[config_constant]
pub const TEST_ARRAY_OF_U8: [u8; 32];

#[config_constant]
pub const TEST_SLICE: &[u8];

#[config_constant]
/// This one has a doc attr
pub const TEST_NESTED_ARRAY: [[u8; 3]; 2];

#[config_constant]
pub const TEST_BOOL: bool;

#[config_constant]
/// This one is not visible
const TEST_STRING: &str;

#[config_bech32_constant]
const TEST_TOKEN_ID: TokenId;

fn main() {
    assert_eq!(TEST_U32, 42);
    assert_eq!(TEST_ARRAY_OF_U8, [11; 32]);
    assert_eq!(TEST_SLICE, &[11; 3]);
    assert_eq!(TEST_NESTED_ARRAY, [[7; 3]; 2]);
    assert_eq!(TEST_BOOL, true);
    assert_eq!(TEST_STRING, "Some Other String");
    assert_eq!(TEST_TOKEN_ID, TokenId::from([0; 32]));
}
