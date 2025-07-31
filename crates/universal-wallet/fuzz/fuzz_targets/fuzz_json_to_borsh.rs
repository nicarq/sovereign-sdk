#![no_main]

use libfuzzer_sys::fuzz_target;
use sov_universal_wallet::schema::Schema;
use universal_wallet_fuzz::FuzzInput;

#[cfg(not(feature = "js-compat"))]
compile_error!("This fuzz test requires JS/JSON compatability otherwise big numbers/floats, etc aren't handled leading to failing tests");

fuzz_target!(|input: FuzzInput| {
    let schema = Schema::of_single_type::<FuzzInput>().unwrap();
    let data = serde_json::to_string(&input).unwrap();
    let json_serialized = schema.json_to_borsh(0, &data).unwrap();
    let borsh = borsh::to_vec(&input).unwrap();

    assert_eq!(json_serialized, borsh);
});
