use std::str::FromStr;

use sov_bank::{CallMessage, Coins, TokenId};
use sov_modules_api::sov_universal_wallet::schema::Schema;
use sov_modules_api::{SafeVec, Spec};
use sov_test_utils::TestSpec;

type S = TestSpec;

#[test]
fn test_create_token() {
    let schema = Schema::of_single_type::<CallMessage<S>>();
    let msg = CallMessage::CreateToken::<S> {
        token_name: "my-token".try_into().unwrap(),
        initial_balance: 100_000_000,
        supply_cap: None,
        mint_to_address: <S as Spec>::Address::from_str(
            "sov1x3jtvq0zwhj2ucsc4hqugskvralrulxvf53vwtkred93s85ar2a",
        )
        .unwrap(),
        admins: SafeVec::new(),
    };

    assert_eq!(schema.display(0, &borsh::to_vec(&msg).unwrap()).unwrap(), "CreateToken { token_name: \"my-token\", initial_balance: 100000000, mint_to_address: sov1x3jtvq0zwhj2ucsc4hqugskvralrulxvf53vwtkred93s85ar2a, admins: [], supply_cap: None }");
}

#[test]
fn test_transfer() {
    let schema = Schema::of_single_type::<CallMessage<S>>();
    let msg: CallMessage<S> = CallMessage::Transfer {
        to: <S as Spec>::Address::from_str(
            "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv",
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

    assert_eq!(schema.display(0, &borsh::to_vec(&msg).unwrap()).unwrap(), "Transfer to address sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv 10000 coins of token ID token_1zut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzutsuzalks.");
}
