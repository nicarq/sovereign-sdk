use simple_nft_module::{
    CallMessage, Event, NonFungibleToken, NonFungibleTokenConfig, OwnerResponse,
};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Error, Spec, TxEffect};
use sov_state::ProverStorage;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{assert_tx_reverted_with_reason, TestRunner};
use sov_test_utils::{
    generate_optimistic_runtime, AsUser, TestStorageSpec, TestUser, TransactionTestCase,
};

pub type S = sov_test_utils::TestSpec;
pub type Storage = ProverStorage<TestStorageSpec>;

generate_optimistic_runtime!(TestNftModuleRuntime <= nft: NonFungibleToken<S>);

pub type RT = TestNftModuleRuntime<S>;

/// Holds the role for the nft tests:
/// - admin: the module admin
/// - owner_0: owns the nft 0
/// - owner_1: owns the nft 1
/// - external_user: a user that is not the owner of the nft
pub struct TestRoles<S: Spec> {
    pub admin: TestUser<S>,
    pub owner_0: TestUser<S>,
    pub owner_1: TestUser<S>,
    pub external_user: TestUser<S>,
}

/// Sets up the test runtime by generating a genesis config with a single nft that has
fn setup() -> (TestRoles<S>, TestRunner<TestNftModuleRuntime<S>, S>) {
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(4);

    let nft_admin = genesis_config.additional_accounts.first().unwrap().clone();
    let owner_0 = genesis_config.additional_accounts[1].clone();
    let owner_1 = genesis_config.additional_accounts[2].clone();
    let external_user = genesis_config.additional_accounts[3].clone();

    let genesis_config = GenesisConfig::from_minimal_config(
        genesis_config.clone().into(),
        NonFungibleTokenConfig {
            admin: nft_admin.address(),
            owners: vec![(0, owner_0.address()), (1, owner_1.address())],
        },
    );

    let runner = TestRunner::new_with_genesis(
        genesis_config.into_genesis_params(),
        TestNftModuleRuntime::default(),
    );

    (
        TestRoles {
            admin: nft_admin,
            owner_0,
            owner_1,
            external_user,
        },
        runner,
    )
}

/// Tries to mint an nft
#[test]
fn mint_succeeds() {
    let (TestRoles { external_user, .. }, mut runner) = setup();

    const NFT_ID: u64 = 2;

    runner.execute_transaction(TransactionTestCase {
        input: external_user
            .create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::Mint { id: NFT_ID }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                TestNftModuleRuntimeEvent::Nft(Event::Mint { id: NFT_ID })
            );

            // Check that the nft is owned by the user
            assert_eq!(
                NonFungibleToken::<S>::default()
                    .get_owner(NFT_ID, state)
                    .unwrap_infallible(),
                OwnerResponse {
                    owner: Some(external_user.address())
                }
            );
        }),
    });
}

/// Tries to mint an nft that already exists
#[test]
fn cannot_mint_twice() {
    let (TestRoles { external_user, .. }, mut runner) = setup();

    const NFT_ID: u64 = 2;

    runner.execute(
        external_user
            .create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::Mint { id: NFT_ID }),
    );

    runner.execute_transaction(TransactionTestCase {
        input: external_user
            .create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::Mint { id: NFT_ID }),
        assert: Box::new(move |result, _state| {
            assert_tx_reverted_with_reason(
                result.tx_receipt,
                anyhow::anyhow!("Token with id {} already exists", NFT_ID),
            );
        }),
    });
}

/// Transfers an nft from one owner to another. Has to be done by the nft owner himself.
#[test]
fn transfer_succeeds() {
    let (
        TestRoles {
            owner_0,
            external_user,
            ..
        },
        mut runner,
    ) = setup();

    const NFT_ID: u64 = 0;

    runner.execute_transaction(TransactionTestCase {
        input: owner_0.create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::Transfer {
            id: NFT_ID,
            to: external_user.address(),
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                TestNftModuleRuntimeEvent::Nft(Event::Transfer { id: NFT_ID })
            );

            assert_eq!(
                NonFungibleToken::<S>::default()
                    .get_owner(NFT_ID, state)
                    .unwrap_infallible(),
                OwnerResponse {
                    owner: Some(external_user.address())
                }
            );
        }),
    });
}

/// Checks that the nft module admin cannot transfer nfts
#[test]
fn admin_cannot_transfer_token() {
    let (TestRoles { admin, .. }, mut runner) = setup();
    const NFT_ID: u64 = 0;

    runner.execute_transaction(TransactionTestCase {
        input: admin.create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::Transfer {
            id: NFT_ID,
            to: admin.address(),
        }),
        assert: Box::new(move |result, _state| {
            if let TxEffect::Reverted(contents) = result.tx_receipt {
                let Error::ModuleError(err) = contents.reason;
                assert_eq!(
                    err.to_string(),
                    "Only token owner can transfer token",
                    "The error message should be \"Only owner can transfer token\""
                );
            } else {
                panic!(
                    "The transaction should have reverted, instead the outcome was {:?}",
                    result.tx_receipt
                );
            }
        }),
    });
}

#[test]
fn other_token_user_cannot_transfer_token() {
    let (TestRoles { owner_1, .. }, mut runner) = setup();
    const NFT_ID: u64 = 0;

    runner.execute_transaction(TransactionTestCase {
        input: owner_1.create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::Transfer {
            id: NFT_ID,
            to: owner_1.address(),
        }),
        assert: Box::new(move |result, _state| {
            assert_tx_reverted_with_reason(
                result.tx_receipt,
                anyhow::anyhow!("Only token owner can transfer token"),
            );
        }),
    });
}

/// Check that one cannot transfer a token that does not exist
#[test]
fn cannot_transfer_non_existent_token() {
    let (TestRoles { owner_0, .. }, mut runner) = setup();
    const NFT_ID: u64 = 42;

    runner.execute_transaction(TransactionTestCase {
        input: owner_0.create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::Transfer {
            id: NFT_ID,
            to: owner_0.address(),
        }),
        assert: Box::new(move |result, _state| {
            assert_tx_reverted_with_reason(
                result.tx_receipt,
                anyhow::anyhow!("Token with id {} does not exist", NFT_ID),
            );
        }),
    });
}

/// Burns an nft successfully
#[test]
fn burn_succeeds() {
    let (TestRoles { owner_0, .. }, mut runner) = setup();
    const NFT_ID: u64 = 0;

    runner.execute_transaction(TransactionTestCase {
        input: owner_0
            .create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::Burn { id: NFT_ID }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                TestNftModuleRuntimeEvent::Nft(Event::Burn { id: NFT_ID })
            );

            assert_eq!(
                NonFungibleToken::<S>::default()
                    .get_owner(NFT_ID, state)
                    .unwrap_infallible(),
                OwnerResponse { owner: None }
            );
        }),
    });
}

#[test]
fn only_owner_can_burn() {
    let (TestRoles { owner_1, .. }, mut runner) = setup();
    const NFT_ID: u64 = 0;

    runner.execute_transaction(TransactionTestCase {
        input: owner_1
            .create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::Burn { id: NFT_ID }),
        assert: Box::new(move |result, _state| {
            assert_tx_reverted_with_reason(
                result.tx_receipt,
                anyhow::anyhow!("Only token owner can burn token"),
            );
        }),
    });
}

#[test]
fn cannot_burn_non_existent_token() {
    let (TestRoles { owner_0, .. }, mut runner) = setup();
    const NFT_ID: u64 = 42;

    runner.execute_transaction(TransactionTestCase {
        input: owner_0
            .create_plain_message::<RT, NonFungibleToken<S>>(CallMessage::Burn { id: NFT_ID }),
        assert: Box::new(move |result, _state| {
            assert_tx_reverted_with_reason(
                result.tx_receipt,
                anyhow::anyhow!("Token with id {} does not exist", NFT_ID),
            );
        }),
    });
}
