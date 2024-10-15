use std::str::FromStr;

use sov_bank::{CallMessage, Coins, TokenId};
use sov_modules_api::sov_universal_wallet::schema::Schema;
use sov_modules_api::Spec;
use sov_test_utils::TestSpec;

type S = TestSpec;

#[test]
fn test_create_token() {
    let schema = Schema::of_single_type::<CallMessage<S>>();
    let msg = CallMessage::CreateToken::<S> {
        token_name: "my-token".to_string(),
        initial_balance: 100_000_000,
        mint_to_address: <S as Spec>::Address::from_str(
            "sov1x3jtvq0zwhj2ucsc4hqugskvralrulxvf53vwtkred93s2x9gmzs04jvyr",
        )
        .unwrap(),
        authorized_minters: vec![],
    };

    assert_eq!(schema.display(0, &borsh::to_vec(&msg).unwrap()).unwrap(), "CreateToken { token_name: \"my-token\", initial_balance: 100000000, mint_to_address: sov1x3jtvq0zwhj2ucsc4hqugskvralrulxvf53vwtkred93s2x9gmzs04jvyr, authorized_minters: [] }");
}

#[test]
fn test_transfer() {
    let schema = Schema::of_single_type::<CallMessage<S>>();
    let msg: CallMessage<S> = CallMessage::Transfer {
        to: <S as Spec>::Address::from_str(
            "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9stup8tx",
        )
        .unwrap(),
        coins: Coins {
            amount: 10_000,
            token_id: TokenId::from_str(
                "token_1zut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzutsuzalks",
            )
            .unwrap(),
        },
    };

    assert_eq!(schema.display(0, &borsh::to_vec(&msg).unwrap()).unwrap(), "Transfer { to: sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9stup8tx, coins: { amount: 10000, token_id: token_1zut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzutsuzalks } }");
}
