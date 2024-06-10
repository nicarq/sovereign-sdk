use sov_bank::{Amount, Bank, CallMessage, Coins, TokenId};
use sov_modules_api::{
    ApiStateAccessor, Context, Module, Spec, StateReader, StateWriter, WorkingSet,
};
use sov_prover_storage_manager::SimpleStorageManager;
use sov_state::namespaces::Accessory;
use sov_state::storage::{SlotKey, SlotValue, StateUpdate};
use sov_state::{ProverStorage, Storage};
use sov_test_utils::TestStorageSpec as StorageSpec;

use crate::helpers::*;

type S = sov_test_utils::TestSpec;

#[test]
fn transfer_initial_token() -> Result<(), anyhow::Error> {
    let initial_balance = 100;
    let bank_config = create_bank_config_with_token(4, initial_balance);
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
    let prover_storage = storage_manager.create_storage();
    let mut state = WorkingSet::new(prover_storage.clone());
    let bank = Bank::default();
    bank.genesis(&bank_config, &mut state).unwrap();

    let token_id = sov_bank::GAS_TOKEN_ID;
    let sender_address = bank_config.gas_token_config.address_and_balances[0].0;
    let sequencer_address = bank_config.gas_token_config.address_and_balances[3].0;
    let receiver_address = bank_config.gas_token_config.address_and_balances[1].0;
    assert_ne!(sender_address, receiver_address);

    let (sender_balance, receiver_balance) = query_sender_receiver_balances(
        &bank,
        token_id,
        sender_address,
        receiver_address,
        &mut state,
    );
    assert_eq!((sender_balance, receiver_balance), (100, 100));
    let prover_storage = commit(state, prover_storage, &mut storage_manager);
    let mut state: WorkingSet<S> = WorkingSet::new(prover_storage.clone());

    transfer(
        &bank,
        token_id,
        sender_address,
        sequencer_address,
        receiver_address,
        10,
        &mut state,
    );
    let (sender_balance, receiver_balance) = query_sender_receiver_balances(
        &bank,
        token_id,
        sender_address,
        receiver_address,
        &mut state,
    );
    assert_eq!((sender_balance, receiver_balance), (90, 110));

    let prover_storage = commit(state, prover_storage, &mut storage_manager);

    let mut state: WorkingSet<S> = WorkingSet::new(prover_storage.clone());

    transfer(
        &bank,
        token_id,
        sender_address,
        sequencer_address,
        receiver_address,
        10,
        &mut state,
    );
    let (sender_balance, receiver_balance) = query_sender_receiver_balances(
        &bank,
        token_id,
        sender_address,
        receiver_address,
        &mut state,
    );
    assert_eq!((sender_balance, receiver_balance), (80, 120));
    let prover_storage = commit(state, prover_storage, &mut storage_manager);

    // Archival tests

    let archival_slot: u64 = 2;
    let state: WorkingSet<S> = WorkingSet::new(prover_storage.clone());
    let mut archival = state.get_archival_at(archival_slot);

    let (sender_balance, receiver_balance) = query_sender_receiver_balances(
        &bank,
        token_id,
        sender_address,
        receiver_address,
        &mut archival,
    );
    assert_eq!((sender_balance, receiver_balance), (90, 110));

    // modify in archival
    transfer(
        &bank,
        token_id,
        sender_address,
        sequencer_address,
        receiver_address,
        5,
        &mut archival,
    );

    let (sender_balance, receiver_balance) = query_sender_receiver_balances(
        &bank,
        token_id,
        sender_address,
        receiver_address,
        &mut archival,
    );
    assert_eq!((sender_balance, receiver_balance), (85, 115));

    let archival_slot: u64 = 1;
    let mut state: WorkingSet<S> = WorkingSet::new(prover_storage.clone());
    let mut archival = state.get_archival_at(archival_slot);
    let (sender_balance, receiver_balance) = query_sender_receiver_balances(
        &bank,
        token_id,
        sender_address,
        receiver_address,
        &mut archival,
    );
    assert_eq!((sender_balance, receiver_balance), (100, 100));

    transfer(
        &bank,
        token_id,
        sender_address,
        sequencer_address,
        receiver_address,
        45,
        &mut archival,
    );

    let (sender_balance, receiver_balance) = query_sender_receiver_balances(
        &bank,
        token_id,
        sender_address,
        receiver_address,
        &mut archival,
    );
    assert_eq!((sender_balance, receiver_balance), (55, 145));

    let (sender_balance, receiver_balance) = query_sender_receiver_balances(
        &bank,
        token_id,
        sender_address,
        receiver_address,
        &mut state,
    );
    assert_eq!((sender_balance, receiver_balance), (80, 120));

    // Accessory tests

    transfer(
        &bank,
        token_id,
        sender_address,
        sequencer_address,
        receiver_address,
        10,
        &mut state,
    );
    let (sender_balance, receiver_balance) = query_sender_receiver_balances(
        &bank,
        token_id,
        sender_address,
        receiver_address,
        &mut state,
    );
    assert_eq!((sender_balance, receiver_balance), (70, 130));

    StateWriter::<Accessory>::set(
        &mut state,
        &SlotKey::from_slice(b"k"),
        SlotValue::from(b"v1".to_vec()),
    )?;
    let val = StateReader::<Accessory>::get(&mut state, &SlotKey::from_slice(b"k"))?.unwrap();
    assert_eq!("v1", String::from_utf8(val.value().to_vec()).unwrap());

    let prover_storage = commit(state, prover_storage, &mut storage_manager);

    // next block

    let mut state: WorkingSet<S> = WorkingSet::new(prover_storage.clone());
    transfer(
        &bank,
        token_id,
        sender_address,
        sequencer_address,
        receiver_address,
        10,
        &mut state,
    );
    let (sender_balance, receiver_balance) = query_sender_receiver_balances(
        &bank,
        token_id,
        sender_address,
        receiver_address,
        &mut state,
    );
    assert_eq!((sender_balance, receiver_balance), (60, 140));
    StateWriter::<Accessory>::set(
        &mut state,
        &SlotKey::from_slice(b"k"),
        SlotValue::from(b"v2".to_vec()),
    )?;
    let val = StateReader::<Accessory>::get(&mut state, &SlotKey::from_slice(b"k"))?.unwrap();
    assert_eq!("v2", String::from_utf8(val.value().to_vec()).unwrap());

    let prover_storage = commit(state, prover_storage, &mut storage_manager);

    // archival versioned state query

    let archival_slot = 3;
    let mut state: ApiStateAccessor<S> = ApiStateAccessor::new(prover_storage.clone());
    let mut archival = state.get_archival_at(archival_slot);
    let val = StateReader::<Accessory>::get(&mut archival, &SlotKey::from_slice(b"k"))?.unwrap();
    assert_eq!("v1", String::from_utf8(val.value().to_vec()).unwrap());

    // archival accessory set

    StateWriter::<Accessory>::set(
        &mut archival,
        &SlotKey::from_slice(b"k"),
        SlotValue::from(b"v3".to_vec()),
    )
    .expect("Should be able to set state");
    let val = StateReader::<Accessory>::get(&mut archival, &SlotKey::from_slice(b"k"))?.unwrap();
    assert_eq!("v3", String::from_utf8(val.value().to_vec()).unwrap());

    let val = StateReader::<Accessory>::get(&mut state, &SlotKey::from_slice(b"k"))?.unwrap();
    assert_eq!("v2", String::from_utf8(val.value().to_vec()).unwrap());

    Ok(())
}

fn query_sender_receiver_balances(
    bank: &Bank<S>,
    token_id: TokenId,
    sender_address: <S as Spec>::Address,
    receiver_address: <S as Spec>::Address,
    state: &mut WorkingSet<S>,
) -> (u64, u64) {
    let sender_balance = bank
        .get_balance_of(&sender_address, token_id, state)
        .unwrap();
    let receiver_balance = bank
        .get_balance_of(&receiver_address, token_id, state)
        .unwrap();
    (sender_balance, receiver_balance)
}

fn transfer(
    bank: &Bank<S>,
    token_id: TokenId,
    sender_address: <S as Spec>::Address,
    sequencer_address: <S as Spec>::Address,
    receiver_address: <S as Spec>::Address,
    transfer_amount: Amount,
    state: &mut WorkingSet<S>,
) {
    let transfer_message = CallMessage::Transfer {
        to: receiver_address,
        coins: Coins {
            amount: transfer_amount,
            token_id,
        },
    };

    let sender_context =
        Context::<S>::new(sender_address, Default::default(), sequencer_address, 1);

    bank.call(transfer_message, &sender_context, state)
        .expect("Transfer call failed");
}

fn commit(
    state: WorkingSet<S>,
    storage: ProverStorage<StorageSpec>,
    storage_manager: &mut SimpleStorageManager<StorageSpec>,
) -> ProverStorage<StorageSpec> {
    // Save checkpoint
    let checkpoint = state.checkpoint();

    let (cache_log, accessory_delta, witness) = checkpoint.0.freeze();

    let (_, mut state_update) = storage
        .compute_state_update(cache_log, &witness)
        .expect("JMT update must succeed");

    state_update.add_accessory_items(accessory_delta.freeze());

    let change_set = storage.materialize_changes(&state_update);
    storage_manager.commit(change_set);
    storage_manager.create_storage()
}
