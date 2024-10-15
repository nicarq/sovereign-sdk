use std::str::FromStr;

use borsh::BorshDeserialize;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::sov_universal_wallet::schema::Schema;
use sov_modules_api::Spec;
use sov_nft::{CallMessage, CollectionId, UserAddress};
use sov_test_utils::TestSpec;

type S = TestSpec;

#[derive(Debug, Clone, PartialEq, borsh::BorshSerialize, BorshDeserialize, UniversalWallet)]
pub enum RuntimeCall {
    Nft(CallMessage<S>),
}

#[test]
fn test_display_nft_createl() {
    let msg: RuntimeCall = RuntimeCall::Nft(CallMessage::CreateCollection {
        name: "Cosmic Crabs".to_string(),
        collection_uri: "https://crab.gang".to_string(),
    });
    let schema = Schema::of_single_type::<RuntimeCall>();
    assert_eq!(
        schema.display(0, &borsh::to_vec(&msg).unwrap()).unwrap(),
        r#"Nft.CreateCollection { name: "Cosmic Crabs", collection_uri: "https://crab.gang" }"#
    );
}

#[test]
fn test_display_nft_mint() {
    let msg: RuntimeCall = RuntimeCall::Nft(CallMessage::MintNft {
        collection_name: "Cosmic Crabs".to_string(),
        token_uri: "https://crab.gang/ferris".to_string(),
        token_id: 1,
        owner: UserAddress::new(
            &<S as Spec>::Address::from_str(
                "sov1x3jtvq0zwhj2ucsc4hqugskvralrulxvf53vwtkred93s2x9gmzs04jvyr",
            )
            .unwrap(),
        ),
        frozen: false,
    });
    let schema = Schema::of_single_type::<RuntimeCall>();
    assert_eq!(
        schema.display(0, &borsh::to_vec(&msg).unwrap()).unwrap(),
        "Nft.MintNft { collection_name: \"Cosmic Crabs\", token_uri: \"https://crab.gang/ferris\", token_id: 1, owner: sov1x3jtvq0zwhj2ucsc4hqugskvralrulxvf53vwtkred93s2x9gmzs04jvyr, frozen: false }"
    );
}

#[test]
fn test_display_nft_transfer() {
    let msg: RuntimeCall = RuntimeCall::Nft(CallMessage::TransferNft {
        collection_id: CollectionId::from([1; 32]),
        token_id: 1,
        to: UserAddress::new(
            &<S as Spec>::Address::from_str(
                "sov1x3jtvq0zwhj2ucsc4hqugskvralrulxvf53vwtkred93s2x9gmzs04jvyr",
            )
            .unwrap(),
        ),
    });
    let schema = Schema::of_single_type::<RuntimeCall>();
    assert_eq!(
        schema.display(0, &borsh::to_vec(&msg).unwrap()).unwrap(),
		"Nft.TransferNft { collection_id: collection1qyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqs2zt6pr, token_id: 1, to: sov1x3jtvq0zwhj2ucsc4hqugskvralrulxvf53vwtkred93s2x9gmzs04jvyr }"
    );
}
