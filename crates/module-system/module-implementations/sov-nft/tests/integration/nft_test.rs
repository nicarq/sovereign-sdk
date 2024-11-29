use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Spec, TxEffect};
use sov_nft::utils::get_collection_id;
use sov_nft::{
    CallMessage, CollectionId, NonFungibleToken, NonFungibleTokenConfig, OwnerAddress, UserAddress,
};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    generate_optimistic_runtime, AsUser, TestSpec, TestUser, TransactionTestCase,
};

pub type S = sov_test_utils::TestSpec;

generate_optimistic_runtime!(TestNftModuleRuntime <= nft: NonFungibleToken<S>);

pub type RT = TestNftModuleRuntime<S>;

const NON_EXISTENT_COLLECTION_NAME: &str = "Non existent collection";
const COLLECTION_NAME: &str = "Test Collection";
const COLLECTION_URI: &str = "http://foo.bar/test_collection";

const TOKEN_ID: u64 = 42;
const TOKEN_URI: &str = "http://foo.bar/test_collection/42";

/// Holds the role for the nft tests:
/// - owner: owns the nft
/// - collection_creator: creates the nft collection
/// - external_user: a user that is not the owner of the nft
pub struct TestRoles<S: Spec> {
    pub nft_owner: TestUser<S>,
    pub external_user: TestUser<S>,
    pub collection_creator: TestUser<S>,
}

/// Sets up the test runtime by generating a genesis config with a single nft that has
fn setup() -> (TestRoles<S>, TestRunner<RT, S>) {
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(4);

    let collection_creator = genesis_config.additional_accounts.first().unwrap().clone();
    let nft_owner = genesis_config.additional_accounts[1].clone();
    let external_user = genesis_config.additional_accounts[2].clone();

    let genesis_config = GenesisConfig::from_minimal_config(
        genesis_config.clone().into(),
        NonFungibleTokenConfig {},
    );

    let runner = TestRunner::new_with_genesis(
        genesis_config.into_genesis_params(),
        TestNftModuleRuntime::default(),
    );

    (
        TestRoles {
            nft_owner,
            external_user,
            collection_creator,
        },
        runner,
    )
}

fn setup_and_create_collection() -> (CollectionId, TestRoles<S>, TestRunner<RT, S>) {
    let (roles, mut runner) = setup();

    let creator = &roles.collection_creator;

    let collection_id = get_collection_id::<S>(COLLECTION_NAME, creator.address().as_ref());

    runner.execute(creator.create_plain_message::<RT, NonFungibleToken<S>>(
        CallMessage::CreateCollection {
            name: COLLECTION_NAME.try_into().unwrap(),
            collection_uri: COLLECTION_URI.try_into().unwrap(),
        },
    ));

    (collection_id, roles, runner)
}

fn setup_and_mint_nft() -> (CollectionId, TestRoles<S>, TestRunner<RT, S>) {
    let (collection_id, roles, mut runner) = setup_and_create_collection();

    let owner = &roles.nft_owner;
    let collection_creator = &roles.collection_creator;

    runner.execute(
        collection_creator.create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::MintNft {
            collection_name: COLLECTION_NAME.try_into().unwrap(),
            token_uri: TOKEN_URI.try_into().unwrap(),
            token_id: TOKEN_ID,
            owner: UserAddress::new(&owner.address()),
            frozen: false,
        }),
    );

    (collection_id, roles, runner)
}

/// Tries to mint an nft
#[test]
fn create_collection_succeeds() {
    let (
        TestRoles {
            nft_owner: creator, ..
        },
        mut runner,
    ) = setup();

    let creator_address: <TestSpec as Spec>::Address = creator.address();
    let collection_id = get_collection_id::<TestSpec>(COLLECTION_NAME, creator_address.as_ref());

    runner.execute_transaction(TransactionTestCase {
        input: creator.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::CreateCollection {
                name: COLLECTION_NAME.try_into().unwrap(),
                collection_uri: COLLECTION_URI.try_into().unwrap(),
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());

            let nft = NonFungibleToken::<S>::default();

            // Check that the nft is owned by the user
            let actual_collection = nft
                .collection(collection_id, state)
                .unwrap_infallible()
                .unwrap();

            assert_eq!(actual_collection.name, COLLECTION_NAME);
            assert_eq!(actual_collection.supply, 0);
            assert_eq!(
                actual_collection.creator.get_address().clone(),
                creator_address
            );
            assert!(!actual_collection.frozen);
        }),
    });
}

#[test]
fn mint_nft_succeeds() {
    let (
        collection_id,
        TestRoles {
            collection_creator,
            nft_owner: owner,
            ..
        },
        mut runner,
    ) = setup_and_create_collection();

    runner.execute_transaction(TransactionTestCase {
        input: collection_creator.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::MintNft {
                collection_name: COLLECTION_NAME.try_into().unwrap(),
                token_uri: TOKEN_URI.try_into().unwrap(),
                token_id: TOKEN_ID,
                owner: UserAddress::new(&owner.address()),
                frozen: false,
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());

            let nft = NonFungibleToken::<S>::default();

            // Check that the nft is owned by the user
            let actual_collection = nft
                .collection(collection_id, state)
                .unwrap_infallible()
                .unwrap();

            assert_eq!(actual_collection.supply, 1);

            let actual_nft = nft
                .nft(collection_id, TOKEN_ID, state)
                .unwrap_infallible()
                .unwrap();
            assert_eq!(actual_nft.token_id, TOKEN_ID);
            assert_eq!(actual_nft.collection_id, collection_id);
            assert_eq!(actual_nft.token_uri, TOKEN_URI.to_string());
            assert_eq!(actual_nft.owner, OwnerAddress::new(&owner.address()));
        }),
    });
}

#[test]
fn mint_nft_to_non_existing_collection_fails() {
    let (
        TestRoles {
            nft_owner: owner, ..
        },
        mut runner,
    ) = setup();

    runner.execute_transaction(TransactionTestCase {
        input: owner.create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::MintNft {
            collection_name: NON_EXISTENT_COLLECTION_NAME.try_into().unwrap(),
            token_uri: TOKEN_URI.try_into().unwrap(),
            token_id: TOKEN_ID,
            owner: UserAddress::new(&owner.address()),
            frozen: false,
        }),
        assert: Box::new(move |result, _| {
            let mint_response = result.tx_receipt;

            if let TxEffect::Reverted(contents) = mint_response {
                match contents.reason {
                    sov_modules_api::Error::ModuleError(anyhow_err) => {
                        let err_message = anyhow_err.to_string();
                        let expected_message = format!(
                            "Collection with name: {} does not exist for creator {}",
                            NON_EXISTENT_COLLECTION_NAME,
                            owner.address()
                        );
                        assert_eq!(err_message, expected_message);
                    }
                }
            } else {
                panic!("Expected an error, got Ok");
            }
        }),
    });
}

#[test]
fn update_a_collection_works() {
    let (
        collection_id,
        TestRoles {
            collection_creator: creator,
            ..
        },
        mut runner,
    ) = setup_and_create_collection();

    let new_collection_uri = "http://new/uri";

    runner.execute_transaction(TransactionTestCase {
        input: creator.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::UpdateCollection {
                name: COLLECTION_NAME.try_into().unwrap(),
                collection_uri: new_collection_uri.try_into().unwrap(),
            },
        ),
        assert: Box::new(move |_, state| {
            let nft = NonFungibleToken::<S>::default();

            let actual_collection = nft
                .collection(collection_id, state)
                .unwrap_infallible()
                .unwrap();
            assert_eq!(
                actual_collection.collection_uri,
                new_collection_uri.to_string()
            );
            assert!(!actual_collection.frozen);
        }),
    });
}

#[test]
/// Freeze a non existent collection
fn freeze_non_existing_collection_fails() {
    let (
        TestRoles {
            collection_creator: creator,
            ..
        },
        mut runner,
    ) = setup();

    runner.execute_transaction(TransactionTestCase {
        input: creator.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::FreezeCollection {
                collection_name: NON_EXISTENT_COLLECTION_NAME.try_into().unwrap(),
            },
        ),
        assert: Box::new(move |receipt, _| {
            let freeze_response = receipt.tx_receipt;

            if let TxEffect::Reverted(inner) = freeze_response {
                match inner.reason {
                    sov_modules_api::Error::ModuleError(anyhow_err) => {
                        let err_message = anyhow_err.to_string();
                        let expected_message = format!(
                            "Collection with name: {} does not exist for creator {}",
                            NON_EXISTENT_COLLECTION_NAME,
                            creator.address()
                        );
                        assert_eq!(err_message, expected_message);
                    }
                }
            } else {
                panic!("Expected an error, got Ok");
            }
        }),
    });
}

/// Freeze collection
#[test]
fn freeze_collection_works() {
    let (
        collection_id,
        TestRoles {
            collection_creator: creator,
            ..
        },
        mut runner,
    ) = setup_and_create_collection();

    runner.execute_transaction(TransactionTestCase {
        input: creator.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::FreezeCollection {
                collection_name: COLLECTION_NAME.try_into().unwrap(),
            },
        ),
        assert: Box::new(move |_, state| {
            let nft = NonFungibleToken::<S>::default();
            let actual_collection = nft
                .collection(collection_id, state)
                .unwrap_infallible()
                .unwrap();

            assert!(actual_collection.frozen);
        }),
    });
}

/// Update collection uri for frozen collection
/// Update a collection
#[test]
fn update_uri_for_frozen_collection_fails() {
    let (
        collection_id,
        TestRoles {
            collection_creator: creator,
            ..
        },
        mut runner,
    ) = setup_and_create_collection();

    runner.execute(creator.create_plain_message::<RT, NonFungibleToken<S>>(
        CallMessage::FreezeCollection {
            collection_name: COLLECTION_NAME.try_into().unwrap(),
        },
    ));

    let updated_collection_uri = "http://new/uri2";

    runner.execute_transaction(TransactionTestCase {
        input: creator.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::UpdateCollection {
                name: COLLECTION_NAME.try_into().unwrap(),
                collection_uri: updated_collection_uri.try_into().unwrap(),
            },
        ),
        assert: Box::new(move |receipt, state| {
            let nft = NonFungibleToken::<S>::default();

            let update_response = receipt.tx_receipt;
            if let TxEffect::Reverted(inner) = update_response {
                match inner.reason {
                    sov_modules_api::Error::ModuleError(anyhow_err) => {
                        let err_message = anyhow_err.to_string();
                        let expected_message = format!(
                            "Collection with name: {} , creator: {} is frozen",
                            COLLECTION_NAME,
                            creator.address()
                        );
                        assert_eq!(err_message, expected_message);
                    }
                }
            } else {
                panic!("Expected an error, got Ok");
            }

            let actual_collection = nft
                .collection(collection_id, state)
                .unwrap_infallible()
                .unwrap();
            assert!(actual_collection.frozen);
            // assert that the collection uri hasn't been changed
            assert_eq!(actual_collection.collection_uri, COLLECTION_URI);
            // assert that supply hasn't been modified
            assert_eq!(actual_collection.supply, 0);
        }),
    });
}

/// transfer NFT with non-owner
#[test]
fn transfer_nft_not_owner_fails() {
    let (
        collection_id,
        TestRoles {
            nft_owner: owner,
            external_user: other_user,
            ..
        },
        mut runner,
    ) = setup_and_mint_nft();

    let target_address = other_user.address();

    runner.execute_transaction(TransactionTestCase {
        input: other_user.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::TransferNft {
                collection_id,
                token_id: TOKEN_ID,
                to: UserAddress::new(&target_address),
            },
        ),
        assert: Box::new(move |receipt, _| {
            let transfer_response = receipt.tx_receipt;
            if let TxEffect::Reverted(inner) = transfer_response {
                match inner.reason {
                    sov_modules_api::Error::ModuleError(anyhow_err) => {
                        let err_message = anyhow_err.to_string();
                        let expected_message =
                            format!(
                            "user: {} does not own nft: {} from collection id: {} , owner is: {}",
                            other_user.address(), TOKEN_ID, collection_id, owner.address()
                        );
                        assert_eq!(err_message, expected_message);
                    }
                }
            } else {
                panic!("Expected an error, got Ok");
            }
        }),
    });
}

#[test]
fn transfer_nft_not_existing_token_id_fails() {
    let (
        collection_id,
        TestRoles {
            nft_owner: owner,
            external_user: other_user,
            ..
        },
        mut runner,
    ) = setup_and_mint_nft();

    let target_address = other_user.address();

    runner.execute_transaction(TransactionTestCase {
        input: owner.create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::TransferNft {
            collection_id,
            token_id: 1000,
            to: UserAddress::new(&target_address),
        }),
        assert: Box::new(move |receipt, _| {
            let transfer_response = receipt.tx_receipt;
            if let TxEffect::Reverted(inner) = transfer_response {
                match inner.reason {
                    sov_modules_api::Error::ModuleError(anyhow_err) => {
                        let err_message = anyhow_err.to_string();
                        let expected_message = format!(
                            "Nft with token_id: {} in collection_id: {} does not exist",
                            1000, collection_id
                        );
                        assert_eq!(err_message, expected_message);
                    }
                }
            } else {
                panic!("Expected an error, got Ok");
            }
        }),
    });
}

#[test]
fn transfer_nft_by_owner_works() {
    let (
        collection_id,
        TestRoles {
            nft_owner: owner,
            external_user: other_user,
            ..
        },
        mut runner,
    ) = setup_and_mint_nft();

    let target_address = other_user.address();

    runner.execute_transaction(TransactionTestCase {
        input: owner.create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::TransferNft {
            collection_id,
            token_id: TOKEN_ID,
            to: UserAddress::new(&target_address),
        }),
        assert: Box::new(move |receipt, state| {
            let transfer_response = receipt.tx_receipt;
            let nft = NonFungibleToken::<S>::default();

            assert!(matches!(transfer_response, TxEffect::Successful(..)));

            let actual_nft = nft
                .nft(collection_id, TOKEN_ID, state)
                .unwrap_infallible()
                .unwrap();
            // ensure token_id didn't change
            assert_eq!(actual_nft.token_id, TOKEN_ID);
            assert_eq!(actual_nft.collection_id, collection_id);
            assert_eq!(actual_nft.token_uri, TOKEN_URI.to_string());
            // ensure that the owner is the new owner
            assert_eq!(actual_nft.owner, OwnerAddress::new(&target_address));

            let actual_collection = nft
                .collection(collection_id, state)
                .unwrap_infallible()
                .unwrap();
            // ensure supply hasn't changed with a transfer
            assert_eq!(actual_collection.supply, 1);
        }),
    });
}

#[test]
fn update_nft_uri() {
    let (
        collection_id,
        TestRoles {
            nft_owner: owner,
            collection_creator,
            ..
        },
        mut runner,
    ) = setup_and_mint_nft();

    let new_token_uri = "http://foo.bar/test_collection/new_url/42";

    runner.execute_transaction(TransactionTestCase {
        input: collection_creator.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::UpdateNft {
                collection_name: COLLECTION_NAME.try_into().unwrap(),
                token_id: TOKEN_ID,
                token_uri: Some(new_token_uri.try_into().unwrap()),
                frozen: None,
            },
        ),
        assert: Box::new(move |receipt, state| {
            let nft = NonFungibleToken::<S>::default();

            let update_response = receipt.tx_receipt;
            assert!(matches!(update_response, TxEffect::Successful(..)));

            let actual_nft = nft
                .nft(collection_id, TOKEN_ID, state)
                .unwrap_infallible()
                .unwrap();

            // ensure token_id didn't change
            assert_eq!(actual_nft.token_id, TOKEN_ID);
            assert_eq!(actual_nft.collection_id, collection_id);
            // token uri should be updated
            assert_eq!(actual_nft.token_uri, new_token_uri.to_string());
            // ensure owner is unchanged
            assert_eq!(actual_nft.owner, OwnerAddress::new(&owner.address()));
            // ensure still unfrozen
            assert!(!actual_nft.frozen);
        }),
    });
}

#[test]
fn freeze_nft() {
    let (
        collection_id,
        TestRoles {
            nft_owner: owner,
            collection_creator,
            ..
        },
        mut runner,
    ) = setup_and_mint_nft();

    runner.execute_transaction(TransactionTestCase {
        input: collection_creator.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::UpdateNft {
                collection_name: COLLECTION_NAME.try_into().unwrap(),
                token_id: TOKEN_ID,
                token_uri: None,
                frozen: Some(true),
            },
        ),
        assert: Box::new(move |receipt, state| {
            let update_response = receipt.tx_receipt;
            let nft = NonFungibleToken::<S>::default();

            assert!(matches!(update_response, TxEffect::Successful(..)));

            let actual_nft = nft
                .nft(collection_id, TOKEN_ID, state)
                .unwrap_infallible()
                .unwrap();
            // ensure token_id didn't change
            assert_eq!(actual_nft.token_id, TOKEN_ID);
            assert_eq!(actual_nft.collection_id, collection_id);
            // token uri should not be updated
            assert_eq!(actual_nft.token_uri, TOKEN_URI.to_string());
            // ensure owner is unchanged
            assert_eq!(actual_nft.owner, OwnerAddress::new(&owner.address()));
            // ensure frozen is true
            assert!(actual_nft.frozen);
        }),
    });
}

/// Update NFT token uri for frozen NFT
#[test]
fn update_token_uri_fails_if_frozen() {
    let (
        collection_id,
        TestRoles {
            collection_creator, ..
        },
        mut runner,
    ) = setup_and_mint_nft();

    // Freeze nft
    runner.execute(
        collection_creator.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::UpdateNft {
                collection_name: COLLECTION_NAME.try_into().unwrap(),
                token_id: TOKEN_ID,
                token_uri: None,
                frozen: Some(true),
            },
        ),
    );

    let new_token_uri = "http://foo.bar/test_collection/new_url/42";

    runner.execute_transaction(TransactionTestCase {
        input: collection_creator.create_plain_message::<RT, NonFungibleToken<S>>(
            CallMessage::UpdateNft {
                collection_name: COLLECTION_NAME.try_into().unwrap(),
                token_id: TOKEN_ID,
                token_uri: Some(new_token_uri.try_into().unwrap()),
                frozen: None,
            },
        ),
        assert: Box::new(move |receipt, state| {
            let update_response = receipt.tx_receipt;

            if let TxEffect::Reverted(inner) = update_response {
                match inner.reason {
                    sov_modules_api::Error::ModuleError(anyhow_err) => {
                        let err_message = anyhow_err.to_string();
                        let expected_message = format!(
                            "NFT with token id {} in collection id {} is frozen",
                            TOKEN_ID, collection_id
                        );
                        assert_eq!(err_message, expected_message);
                    }
                }
            } else {
                panic!("Expected an error, got Ok");
            }

            let nft = NonFungibleToken::<S>::default();
            // ensure that token uri is unchanged
            let actual_nft = nft
                .nft(collection_id, TOKEN_ID, state)
                .unwrap_infallible()
                .unwrap();
            // token uri should be unchanged.
            assert_eq!(actual_nft.token_uri, TOKEN_URI.to_string());
        }),
    });
}

#[test]
fn can_still_transfer_frozen_nft() {
    let (
        collection_id,
        TestRoles {
            nft_owner: owner,
            external_user: other_user,
            ..
        },
        mut runner,
    ) = setup_and_mint_nft();

    // Freeze nft
    runner.execute(
        owner.create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::UpdateNft {
            collection_name: COLLECTION_NAME.try_into().unwrap(),
            token_id: TOKEN_ID,
            token_uri: None,
            frozen: Some(true),
        }),
    );

    let target_address = other_user.address();

    runner.execute_transaction(TransactionTestCase {
        input: owner.create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::TransferNft {
            collection_id,
            token_id: TOKEN_ID,
            to: UserAddress::new(&target_address),
        }),
        assert: Box::new(move |receipt, state| {
            let transfer_response = receipt.tx_receipt;
            let nft = NonFungibleToken::<S>::default();

            assert!(matches!(transfer_response, TxEffect::Successful(..)));

            let actual_nft = nft
                .nft(collection_id, TOKEN_ID, state)
                .unwrap_infallible()
                .unwrap();
            // ensure token_id didn't change
            assert_eq!(actual_nft.token_id, TOKEN_ID);
            assert_eq!(actual_nft.collection_id, collection_id);
            // token uri should be token_uri
            assert_eq!(actual_nft.token_uri, TOKEN_URI.to_string());
            // ensure that the owner is the new owner
            assert_eq!(actual_nft.owner, OwnerAddress::new(&target_address));

            let actual_collection = nft
                .collection(collection_id, state)
                .unwrap_infallible()
                .unwrap();
            // ensure supply hasn't changed with a transfer
            assert_eq!(actual_collection.supply, 1);
        }),
    });
}
