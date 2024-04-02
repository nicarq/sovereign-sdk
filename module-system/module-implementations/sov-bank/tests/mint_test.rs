use sov_bank::{
    get_token_id, Bank, BankConfig, CallMessage, Coins, GasTokenConfig, TokenId,
    TotalSupplyResponse, GAS_TOKEN_ID,
};
use sov_modules_api::utils::generate_address;
use sov_modules_api::{Address, Context, Error, Module, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::{DefaultStorageSpec, ProverStorage};

mod helpers;

type S = sov_test_utils::TestSpec;
pub type Storage = ProverStorage<DefaultStorageSpec>;

#[test]
fn mint_token() {
    let bank = Bank::<S>::default();
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());

    let minter_address = generate_address::<S>("minter");
    let sequencer_address = generate_address::<S>("sequencer");
    let minter_context = Context::<S>::new(minter_address, sequencer_address, 1);

    let token_name = "Token1".to_owned();
    let initial_balance = 100;
    let token_id = GAS_TOKEN_ID;

    let bank_config = BankConfig::<S> {
        gas_token_config: GasTokenConfig {
            token_name: token_name.clone(),
            address_and_balances: vec![(minter_address, initial_balance)],
            authorized_minters: vec![minter_address],
        },

        tokens: vec![],
    };
    bank.genesis(&bank_config, &mut working_set).unwrap();

    let query_total_supply = |token_id: TokenId, working_set: &mut WorkingSet<S>| -> Option<u64> {
        let total_supply: TotalSupplyResponse =
            bank.supply_of(None, token_id, working_set).unwrap();
        total_supply.amount
    };

    let query_user_balance =
        |user_address: Address, working_set: &mut WorkingSet<S>| -> Option<u64> {
            bank.get_balance_of(&user_address, token_id, working_set)
        };

    let previous_total_supply = query_total_supply(token_id, &mut working_set);
    assert_eq!(Some(initial_balance), previous_total_supply);

    // -----
    // Mint Additional
    let mint_amount = 10;
    let new_holder = generate_address::<S>("new_holder");
    let mint_message = CallMessage::Mint {
        coins: Coins {
            amount: mint_amount,
            token_id,
        },
        minter_address: new_holder,
    };

    let _minted = bank
        .call(mint_message.clone(), &minter_context, &mut working_set)
        .expect("Failed to mint token");

    let total_supply = query_total_supply(token_id, &mut working_set);
    assert_eq!(Some(initial_balance + mint_amount), total_supply);

    // check user balance after minting
    let balance = query_user_balance(new_holder, &mut working_set);
    assert_eq!(Some(10), balance);

    // check original token creation balance
    let bal = query_user_balance(minter_address, &mut working_set);
    assert_eq!(Some(100), bal);

    // Mint with an un-authorized user
    let unauthorized_address = generate_address::<S>("unauthorized_address");
    let sequencer_address = generate_address::<S>("sequencer");
    let unauthorized_context = Context::<S>::new(unauthorized_address, sequencer_address, 1);
    let unauthorized_mint = bank.call(mint_message, &unauthorized_context, &mut working_set);

    assert!(unauthorized_mint.is_err());

    let Error::ModuleError(err) = unauthorized_mint.err().unwrap();
    let mut chain = err.chain();

    let message_1 = chain.next().unwrap().to_string();
    let message_2 = chain.next().unwrap().to_string();
    assert!(chain.next().is_none());

    assert_eq!(
        format!(
            "Failed mint coins(token_id={} amount={}) to {} by authorizer {}",
            token_id, mint_amount, new_holder, unauthorized_address
        ),
        message_1
    );
    assert_eq!(
        format!(
            "Sender {} is not an authorized minter of token {}",
            unauthorized_address, token_name,
        ),
        message_2
    );

    // Authorized minter test
    let salt = 0;
    let token_name = "Token_New".to_owned();
    let initial_balance = 100;
    let token_id = get_token_id::<S>(&token_name, &minter_address, salt);
    let authorized_minter_address_1 = generate_address::<S>("authorized_minter_1");
    let authorized_minter_address_2 = generate_address::<S>("authorized_minter_2");
    let sequencer_address = generate_address::<S>("sequencer");
    // ---
    // Deploying token
    let mint_message = CallMessage::CreateToken {
        salt,
        token_name: token_name.clone(),
        initial_balance,
        minter_address,
        authorized_minters: vec![authorized_minter_address_1, authorized_minter_address_2],
    };
    let _minted = bank
        .call(mint_message, &minter_context, &mut working_set)
        .expect("Failed to mint token");
    assert_eq!(working_set.events().len(), 1);

    // Try to mint new token with original token creator, in this case minter_context
    let mint_amount = 10;
    let new_holder = generate_address::<S>("new_holder_2");
    let mint_message = CallMessage::Mint {
        coins: Coins {
            amount: mint_amount,
            token_id,
        },
        minter_address: new_holder,
    };

    let minted = bank.call(mint_message, &minter_context, &mut working_set);
    assert!(minted.is_err());
    let Error::ModuleError(err) = minted.err().unwrap();
    let mut chain = err.chain();

    let message_1 = chain.next().unwrap().to_string();
    let message_2 = chain.next().unwrap().to_string();
    assert!(chain.next().is_none());
    assert_eq!(
        format!(
            "Failed mint coins(token_id={} amount={}) to {} by authorizer {}",
            token_id, mint_amount, new_holder, minter_address,
        ),
        message_1
    );
    assert_eq!(
        format!(
            "Sender {} is not an authorized minter of token {}",
            minter_address, token_name
        ),
        message_2
    );
    // Try to mint new token with authorized sender 2
    let authorized_minter_2_context =
        Context::<S>::new(authorized_minter_address_2, sequencer_address, 1);
    let mint_message = CallMessage::Mint {
        coins: Coins {
            amount: mint_amount,
            token_id,
        },
        minter_address: new_holder,
    };

    let _minted = bank
        .call(mint_message, &authorized_minter_2_context, &mut working_set)
        .expect("Failed to mint token");
    let supply = query_total_supply(token_id, &mut working_set);
    assert_eq!(working_set.events().len(), 1);
    assert_eq!(Some(110), supply);

    // Try to mint new token with authorized sender 1
    let authorized_minter_1_context =
        Context::<S>::new(authorized_minter_address_1, sequencer_address, 1);
    let mint_message = CallMessage::Mint {
        coins: Coins {
            amount: mint_amount,
            token_id,
        },
        minter_address: new_holder,
    };

    let _minted = bank
        .call(mint_message, &authorized_minter_1_context, &mut working_set)
        .expect("Failed to mint token");
    let supply = query_total_supply(token_id, &mut working_set);
    assert_eq!(working_set.events().len(), 1);
    assert_eq!(Some(120), supply);

    // Overflow test - account balance
    let overflow_mint_message = CallMessage::Mint {
        coins: Coins {
            amount: u64::MAX,
            token_id,
        },
        minter_address: new_holder,
    };

    let minted = bank.call(
        overflow_mint_message,
        &authorized_minter_1_context,
        &mut working_set,
    );
    assert!(minted.is_err());
    let Error::ModuleError(err) = minted.err().unwrap();
    let mut chain = err.chain();
    let message_1 = chain.next().unwrap().to_string();
    let message_2 = chain.next().unwrap().to_string();
    assert!(chain.next().is_none());
    assert_eq!(
        format!(
            "Failed mint coins(token_id={} amount={}) to {} by authorizer {}",
            token_id,
            u64::MAX,
            new_holder,
            authorized_minter_address_1,
        ),
        message_1
    );
    assert_eq!(
        "Account balance overflow in the mint method of bank module",
        message_2,
    );
    // assert that the supply is unchanged after the overflow mint
    let supply = query_total_supply(token_id, &mut working_set);
    assert_eq!(Some(120), supply);

    // Overflow test 2 - total supply
    let new_holder = generate_address::<S>("new_holder_3");
    let overflow_mint_message = CallMessage::Mint {
        coins: Coins {
            amount: u64::MAX - 1,
            token_id,
        },
        minter_address: new_holder,
    };

    let minted = bank.call(
        overflow_mint_message,
        &authorized_minter_1_context,
        &mut working_set,
    );
    assert!(minted.is_err());
    let Error::ModuleError(err) = minted.err().unwrap();
    let mut chain = err.chain();
    let message_1 = chain.next().unwrap().to_string();
    let message_2 = chain.next().unwrap().to_string();
    assert!(chain.next().is_none());
    assert_eq!(
        format!(
            "Failed mint coins(token_id={} amount={}) to {} by authorizer {}",
            token_id,
            u64::MAX - 1,
            new_holder,
            authorized_minter_address_1,
        ),
        message_1
    );
    assert_eq!(
        "Total Supply overflow in the mint method of bank module",
        message_2,
    );

    // assert that the supply is unchanged after the overflow mint
    let supply = query_total_supply(token_id, &mut working_set);
    assert_eq!(Some(120), supply);
}
