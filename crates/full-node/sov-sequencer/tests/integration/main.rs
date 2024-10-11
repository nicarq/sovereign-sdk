use sov_modules_api::transaction::Transaction;
use sov_modules_api::{CryptoSpec, Spec};
use sov_rollup_interface::reexports::digest::Digest;
use sov_rollup_interface::TxHash;
use sov_test_utils::generators::bank::BankMessageGenerator;
use sov_test_utils::runtime::TestOptimisticRuntime;
use sov_test_utils::{MessageGenerator, TestPrivateKey, TestSpec};

mod rest_api;
mod std_batch_builder;
mod websockets;

/// Generates a hanful of transactions and returns the hash of the first one.
pub fn generate_txs(admin_private_key: TestPrivateKey) -> Vec<(TxHash, Transaction<TestSpec>)> {
    let bank_generator =
        BankMessageGenerator::<TestSpec>::with_minter_and_transfer(admin_private_key);
    let messages_iter = bank_generator.create_default_messages().into_iter();

    let mut txs = Vec::default();
    for message in messages_iter {
        let tx = message.to_tx::<TestOptimisticRuntime<TestSpec>>();
        let tx_hash = TxHash::new(
            <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::Hasher::digest(
                borsh::to_vec(&tx).unwrap(),
            )
            .into(),
        );

        txs.push((tx_hash, tx));
    }

    txs
}
