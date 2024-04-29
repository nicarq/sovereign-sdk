use sov_bank::{
    get_token_id, Bank, BankConfig, CallMessage, Coins, GasTokenConfig, TokenId,
    TotalSupplyResponse, GAS_TOKEN_ID,
};
use sov_modules_api::utils::generate_address;
use sov_modules_api::{Context, Error, Module, Spec, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::{DefaultStorageSpec, ProverStorage};

mod helpers;
type S = sov_test_utils::TestSpec;
pub type Storage = ProverStorage<DefaultStorageSpec>;

#[test]
fn freeze_token() {
    let bank = Bank::<S>::default();
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());

    let minter_address = generate_address::<S>("minter");
    let sequencer_address = generate_address::<S>("sequencer");
    let minter_context = Context::<S>::new(minter_address, sequencer_address, 1);

    let salt = 0;
    let token_name = "Token1".to_owned();
    let initial_balance = 100;
    let token_id = GAS_TOKEN_ID;

    let bank_config = BankConfig::<S> {
        gas_token_config: GasTokenConfig {
            token_name: token_name.clone(),
            authorized_minters: vec![minter_address],
            address_and_balances: vec![(minter_address, initial_balance)],
        },
        tokens: vec![],
    };
    bank.genesis(&bank_config, &mut working_set).unwrap();

    // -----
    // Freeze
    let freeze_message = CallMessage::Freeze { token_id };

    let _freeze = bank
        .call(freeze_message, &minter_context, &mut working_set)
        .expect("Failed to freeze token");
    assert_eq!(working_set.events().len(), 1);

    // ----
    // Try to freeze an already frozen token
    let freeze_message = CallMessage::Freeze { token_id };

    let freeze = bank.call(freeze_message, &minter_context, &mut working_set);
    assert!(freeze.is_err());
    let Error::ModuleError(err) = freeze.err().unwrap();
    let mut chain = err.chain();
    let message_1 = chain.next().unwrap().to_string();
    let message_2 = chain.next().unwrap().to_string();
    assert!(chain.next().is_none());
    assert_eq!(
        format!(
            "Failed freeze token_id={} by sender {}",
            token_id, minter_address
        ),
        message_1
    );
    assert_eq!(format!("Token {} is already frozen", token_name), message_2);

    // create a second token
    let token_name_2 = "Token2".to_owned();
    let initial_balance = 100;
    let token_id_2 = get_token_id::<S>(&token_name_2, &minter_address, salt);

    // ---
    // Deploying second token
    let mint_message = CallMessage::CreateToken {
        salt,
        token_name: token_name_2.clone(),
        initial_balance,
        minter_address,
        authorized_minters: vec![minter_address],
    };
    let _minted = bank
        .call(mint_message, &minter_context, &mut working_set)
        .expect("Failed to mint token");
    // Two create token events should be present because of the second create token above
    assert_eq!(working_set.events().len(), 2);

    // Try to freeze with a non authorized minter
    let unauthorized_address = generate_address::<S>("unauthorized_address");
    let sequencer_address = generate_address::<S>("sequencer");
    let unauthorized_context = Context::<S>::new(unauthorized_address, sequencer_address, 1);
    let freeze_message = CallMessage::Freeze {
        token_id: token_id_2,
    };

    let freeze = bank.call(freeze_message, &unauthorized_context, &mut working_set);
    assert!(freeze.is_err());
    let Error::ModuleError(err) = freeze.err().unwrap();
    let mut chain = err.chain();
    let message_1 = chain.next().unwrap().to_string();
    let message_2 = chain.next().unwrap().to_string();
    assert!(chain.next().is_none());
    assert_eq!(
        format!(
            "Failed freeze token_id={} by sender {}",
            token_id_2, unauthorized_address
        ),
        message_1
    );
    assert_eq!(
        format!(
            "Sender {} is not an authorized minter of token {}",
            unauthorized_address, token_name_2
        ),
        message_2
    );

    // Try to mint a frozen token
    let mint_amount = 10;
    let new_holder = generate_address::<S>("new_holder");
    let mint_message = CallMessage::Mint {
        coins: Coins {
            amount: mint_amount,
            token_id,
        },
        minter_address: new_holder,
    };

    let query_total_supply = |token_id: TokenId, working_set: &mut WorkingSet<S>| -> Option<u64> {
        let total_supply: TotalSupplyResponse =
            bank.supply_of(None, token_id, working_set).unwrap();
        total_supply.amount
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
            token_id, mint_amount, new_holder, minter_address
        ),
        message_1
    );
    assert_eq!(
        format!("Attempt to mint frozen token {}", token_name),
        message_2
    );

    // -----
    // Try to mint an unfrozen token, sanity check
    let mint_amount = 10;
    let mint_message = CallMessage::Mint {
        coins: Coins {
            amount: mint_amount,
            token_id: token_id_2,
        },
        minter_address,
    };

    let _minted = bank
        .call(mint_message, &minter_context, &mut working_set)
        .expect("Failed to mint token");
    assert_eq!(working_set.events().len(), 2);

    let total_supply = query_total_supply(token_id_2, &mut working_set);
    assert_eq!(Some(initial_balance + mint_amount), total_supply);

    let query_user_balance =
        |token_id: TokenId,
         user_address: <S as Spec>::Address,
         working_set: &mut WorkingSet<S>|
         -> Option<u64> { bank.get_balance_of(&user_address, token_id, working_set) };
    let bal = query_user_balance(token_id_2, minter_address, &mut working_set);

    assert_eq!(Some(110), bal);
}
