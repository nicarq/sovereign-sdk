use std::fs;
use std::path::{Path, PathBuf};

use borsh::{BorshDeserialize, BorshSerialize};
use demo_stf::runtime::{Runtime, RuntimeCall, RuntimeSubcommand};
use sov_cli::wallet_state::WalletState;
use sov_cli::workflows::transactions::{ImportTransaction, TransactionWorkflow};
use sov_mock_da::MockDaSpec;
use sov_modules_api::cli::{FileNameArg, JsonStringArg};
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{CryptoSpec, PrivateKey, Spec};
use sov_test_utils::TestSpec;
type Da = MockDaSpec;

#[test]
fn test_import_transaction_from_string() {
    let app_dir = tempfile::tempdir().unwrap();
    let mut wallet_state = WalletState::<RuntimeCall<TestSpec, Da>, TestSpec>::default();

    let test_token_path = make_test_path("requests/create_token.json");
    let subcommand = RuntimeSubcommand::<JsonStringArg, TestSpec, Da>::bank {
        contents: JsonStringArg {
            json: std::fs::read_to_string(test_token_path).unwrap(),
            chain_id: 0,
            max_priority_fee: 0,
            max_fee: 0,
            gas_limit: None,
        },
    };

    let workflow = TransactionWorkflow::Import(ImportTransaction::<
        _,
        RuntimeSubcommand<JsonStringArg, TestSpec, Da>,
    >::FromFile(subcommand));
    workflow
        .run::<Runtime<TestSpec, Da>, _, _, _, _, _>(&mut wallet_state, app_dir)
        .unwrap();

    assert_eq!(wallet_state.unsent_transactions.len(), 1);
}

#[test]
fn test_import_transaction_from_file() {
    let app_dir = tempfile::tempdir().unwrap();
    let mut wallet_state = WalletState::<RuntimeCall<TestSpec, Da>, TestSpec>::default();

    let test_token_path = make_test_path("requests/create_token.json");
    let subcommand = RuntimeSubcommand::<FileNameArg, TestSpec, Da>::bank {
        contents: FileNameArg {
            path: test_token_path.to_str().unwrap().into(),
            chain_id: 0,
            max_priority_fee: 0,
            max_fee: 0,
            gas_limit: None,
        },
    };

    let workflow = TransactionWorkflow::Import(ImportTransaction::<
        _,
        RuntimeSubcommand<JsonStringArg, TestSpec, Da>,
    >::FromFile(subcommand));
    workflow
        .run::<Runtime<TestSpec, Da>, _, _, _, _, _>(&mut wallet_state, app_dir)
        .unwrap();

    assert_eq!(wallet_state.unsent_transactions.len(), 1);
}

#[test]
fn transaction_is_serialized_correctly() {
    let mut wallet_state = WalletState::<RuntimeCall<TestSpec, Da>, TestSpec>::default();

    let runtime_call_path = make_test_path("requests/create_token.json");
    let runtime_call_json = fs::read_to_string(runtime_call_path).unwrap();
    let runtime_call_bank = serde_json::from_str(&runtime_call_json).unwrap();
    let runtime_call = RuntimeCall::bank(runtime_call_bank);
    let runtime_call_bytes = runtime_call.try_to_vec().unwrap();

    let chain_id = 0;
    let max_priority_fee = PriorityFeeBips::ZERO;
    let max_fee = 0;
    let gas_limit = None;
    let unsigned_tx = UnsignedTransaction::new(
        runtime_call,
        chain_id,
        max_priority_fee,
        max_fee,
        gas_limit.clone(),
    );

    wallet_state.unsent_transactions.push(unsigned_tx);

    let key = <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate();
    let initial_nonce = 15;
    let txs = wallet_state.take_signed_transactions(&key, initial_nonce);

    for (i, tx) in txs.into_iter().enumerate() {
        let tx = Transaction::<TestSpec>::try_from_slice(&tx).unwrap();
        let tx_p = Transaction::<TestSpec>::new_signed_tx(
            &key,
            runtime_call_bytes.clone(),
            chain_id,
            max_priority_fee,
            max_fee,
            gas_limit.clone(),
            initial_nonce + i as u64,
        );

        tx.verify().expect("the computed signature is incorrect");

        assert_eq!(
            tx, tx_p,
            "the stored transaction doesn't match the expected data"
        );
    }
}

fn make_test_path<P: AsRef<Path>>(path: P) -> PathBuf {
    let mut sender_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    sender_path.push("test-data");

    sender_path.push(path);

    sender_path
}
