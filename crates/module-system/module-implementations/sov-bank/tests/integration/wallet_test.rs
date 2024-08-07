use std::str::FromStr;

use sov_bank::CallMessage;
use sov_modules_api::sov_wallet_format::compiled_schema::CompiledSchema;
use sov_modules_api::Spec;
use sov_test_utils::TestSpec;

type S = TestSpec;

#[test]
fn test_create_token() {
    let schema = CompiledSchema::of::<CallMessage<S>>();
    let msg = CallMessage::CreateToken::<S> {
        salt: 11,
        token_name: "my-token".to_string(),
        initial_balance: 100_000_000,
        mint_to_address: <S as Spec>::Address::from_str(
            "sov1x3jtvq0zwhj2ucsc4hqugskvralrulxvf53vwtkred93s2x9gmzs04jvyr",
        )
        .unwrap(),
        authorized_minters: vec![],
    };

    assert_eq!(schema.display(&borsh::to_vec(&msg).unwrap()).unwrap(), "CreateToken { salt: 11, token_name: \"my-token\", initial_balance: 100000000, mint_to_address: sov1x3jtvq0zwhj2ucsc4hqugskvralrulxvf53vwtkred93s2x9gmzs04jvyr, authorized_minters: []}");
}
