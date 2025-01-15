use std::env;

use helpers::*;
use sov_attester_incentives::AttesterIncentives;
use sov_bank::IntoPayable;
use sov_mock_da::MockBlob;
use sov_modules_api::capabilities::TransactionAuthenticator;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction};
use sov_modules_api::{ApiStateAccessor, Gas, GasArray, GasSpec, ModuleInfo, RawTx, Rewards};
use sov_modules_stf_blueprint::TxEffect;
use sov_rollup_interface::da::RelevantBlobs;

use super::{get_balance, get_seq_bond, TxStatus};
use crate::stf_blueprint::setup;

type S = sov_test_utils::TestSpec;

fn check_txs(tx_statuses: Vec<TxStatus>, priority_fee_bips: PriorityFeeBips) {
    let (mut runner, users, sequencer_account) = setup(2);

    let actors = Actors {
        admin_account: users[0].clone(),
        not_admin_account: users[1].clone(),
        sequencer_account,
    };

    let start = runner.query_visible_state(|state| actors.balances(state));

    let txs_len = tx_statuses.len();

    let mock_blob = create_txs(
        &tx_statuses,
        priority_fee_bips,
        &actors.admin_account,
        &actors.not_admin_account,
        runner.config.sequencer_da_address,
    );

    let blobs = RelevantBlobs {
        proof_blobs: Default::default(),
        batch_blobs: vec![mock_blob],
    };

    {
        let result = runner.execute::<RelevantBlobs<MockBlob>>(blobs);
        let batch_receipt = result.batch_receipts[0].clone();

        let gas_price = &batch_receipt.inner.gas_price;
        let tx_receipts = &batch_receipt.tx_receipts;
        let ignored_tx_receipts = &batch_receipt.ignored_tx_receipts;

        assert_eq!(tx_receipts.len() + ignored_tx_receipts.len(), txs_len);

        let mut seq_fee = 0;
        let mut seq_penalty = 0;
        let mut gas_value_charged_to_user = 0;

        let mut total_gas = <S as GasSpec>::Gas::ZEROED;
        for tx_receipt in tx_receipts {
            match &tx_receipt.receipt {
                TxEffect::Successful(tx_contents) => {
                    total_gas = total_gas.checked_combine(&tx_contents.gas_used).unwrap();
                    let gas_value = tx_contents.gas_used.value(gas_price);
                    gas_value_charged_to_user += gas_value;
                    seq_fee += priority_fee_bips.apply(gas_value).unwrap();
                }
                TxEffect::Skipped(tx_contents) => {
                    total_gas = total_gas.checked_combine(&tx_contents.gas_used).unwrap();
                    let gas_value = tx_contents.gas_used.value(gas_price);
                    // Sequencer doesn't get the fee and is penalized
                    seq_penalty += gas_value;
                }
                TxEffect::Reverted(tx_contents) => {
                    total_gas = total_gas.checked_combine(&tx_contents.gas_used).unwrap();
                    // From gas usage point of view the `Successful & Reverted` cases are the same.
                    let gas_value = tx_contents.gas_used.value(gas_price);
                    gas_value_charged_to_user += gas_value;
                    seq_fee += priority_fee_bips.apply(gas_value).unwrap();
                }
            }
        }

        for ignored_tx_receipt in ignored_tx_receipts {
            let ignored = &ignored_tx_receipt.ignored;
            let gas_used = &ignored.gas_used;
            total_gas = total_gas.checked_combine(gas_used).unwrap();
            let gas_value = gas_used.value(gas_price);
            seq_penalty += gas_value;
        }

        let end = runner.query_state(|state| actors.balances(state));

        // Check user balances.
        assert_eq!(
            end.admin_balance + end.not_admin_balance,
            start.admin_balance + start.not_admin_balance - seq_fee - gas_value_charged_to_user
        );

        // Check sequencer rewards.
        assert_eq!(
            end.sequencer_bond,
            start.sequencer_bond + seq_fee - seq_penalty
        );

        // Check prover rewards.
        assert_eq!(
            end.attester_module_balance,
            start.attester_module_balance + gas_value_charged_to_user + seq_penalty
        );

        // This has already been tested by previous assertions, but here we explicitly clarify that no money is created or lost.
        assert_eq!(end.total_balance(), start.total_balance());

        assert_eq!(
            batch_receipt.inner.outcome,
            sov_modules_api::BatchSequencerOutcome {
                rewards: Rewards {
                    accumulated_reward: seq_fee,
                    accumulated_penalty: seq_penalty,
                }
            }
        );

        assert_eq!(batch_receipt.inner.gas_used, total_gas);
    }
}

// Execute batch of valid transactions and ensure that the relevant balances ware updated correctly
#[test]
fn execute_many_successful_tx_test() {
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
    ];
    check_txs(tx_statuses, priority_fee_bips);
}

// Execute a batch of mixed transactions and ensure that the relevant balances were updated correctly
#[test]
fn execute_batch_of_valid_and_invalid_tx_test() {
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![
        TxStatus::BadSerialization,
        TxStatus::SignerDoesNotExist,
        TxStatus::Success,
        TxStatus::BadSignature,
        TxStatus::Success,
        TxStatus::BadChainId,
        TxStatus::BadGeneration,
        TxStatus::Success,
        TxStatus::Reverted,
    ];
    check_txs(tx_statuses, priority_fee_bips);
}

// Execute a batch of invalid transactions and ensure that the relevant balances ware updated correctly
#[test]
fn execute_batch_of_invalid_tx_test() {
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    // BadGeneration is only possible if an account already had at least one valid tx, so we cannot
    // test it here
    let tx_statuses = vec![
        TxStatus::OutOfGas,
        TxStatus::BadChainId,
        TxStatus::BadChainId,
        TxStatus::BadSignature,
        TxStatus::SignerDoesNotExist,
        TxStatus::BadChainId,
        TxStatus::OutOfGas,
        TxStatus::BadSignature,
    ];
    check_txs(tx_statuses, priority_fee_bips);
}

// The batch from an unregistered sequencer is ignored, and no batch receipt is returned.
#[test]
fn non_existing_seq_da_tests() {
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![TxStatus::Success];

    let (mut runner, users, sequencer_account) = setup(2);

    let actors = Actors {
        admin_account: users[0].clone(),
        not_admin_account: users[1].clone(),
        sequencer_account,
    };

    let bad_da_address: [u8; 32] = [33u8; 32];

    let mock_blob = create_txs(
        &tx_statuses,
        priority_fee_bips,
        &actors.admin_account,
        &actors.not_admin_account,
        bad_da_address.into(),
    );

    let blobs = RelevantBlobs {
        proof_blobs: Default::default(),
        batch_blobs: vec![mock_blob],
    };

    let result = runner.execute::<RelevantBlobs<MockBlob>>(blobs);
    assert!(result.batch_receipts.is_empty());
}

#[test]
fn sequencer_run_out_of_gas() {
    env::set_var(
        "SOV_SDK_CONST_OVERRIDE_DEFAULT_GAS_TO_CHARGE_PER_BYTE_BORSH_DESERIALIZATION",
        "[100000, 100000]",
    );

    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![TxStatus::Success];
    check_txs(tx_statuses, priority_fee_bips);
}

mod helpers {
    use sov_modules_api::macros::config_value;
    use sov_modules_api::transaction::PriorityFeeBips;
    use sov_modules_api::{DaSpec, FullyBakedTx, Spec};
    use sov_test_utils::{EncodeCall, TestSequencer, TestUser};
    use sov_value_setter::{CallMessage, ValueSetter};

    use super::super::IntegTestRuntime;
    use super::*;
    use crate::stf_blueprint::{
        create_tx_bad_sender, create_tx_bad_sig, create_tx_out_of_gas, create_tx_valid,
        IntegTestRuntimeCall,
    };

    pub(crate) struct Actors {
        pub(crate) admin_account: TestUser<S>,
        pub(crate) not_admin_account: TestUser<S>,
        pub(crate) sequencer_account: TestSequencer<S>,
    }

    impl Actors {
        pub(crate) fn balances(&self, state: &mut ApiStateAccessor<S>) -> Balances {
            let attester_module = AttesterIncentives::<S>::default();
            Balances {
                admin_balance: get_balance(&self.admin_account.address(), state),
                not_admin_balance: get_balance(&self.not_admin_account.address(), state),
                attester_module_balance: get_balance(attester_module.id().to_payable(), state),
                sequencer_bond: get_seq_bond(&self.sequencer_account.da_address, state).unwrap(),
            }
        }
    }

    #[derive(Debug, Eq, PartialEq)]
    pub(crate) struct Balances {
        pub(crate) admin_balance: u64,
        pub(crate) not_admin_balance: u64,
        pub(crate) attester_module_balance: u64,
        pub(crate) sequencer_bond: u64,
    }

    impl Balances {
        pub(crate) fn total_balance(&self) -> u64 {
            self.admin_balance
                + self.not_admin_balance
                + self.sequencer_bond
                + self.attester_module_balance
        }
    }

    pub(crate) fn create_txs(
        statuses: &[TxStatus],
        max_priority_fee_bips: PriorityFeeBips,
        admin: &TestUser<S>,
        not_admin: &TestUser<S>,
        seq_da_address: <<S as Spec>::Da as DaSpec>::Address,
    ) -> MockBlob {
        let mut generation = 10;
        let mut txs = Vec::new();
        for status in statuses {
            match status {
                TxStatus::Success => {
                    let tx = create_tx_valid(
                        generation,
                        max_priority_fee_bips,
                        admin,
                        config_value!("CHAIN_ID"),
                        encode_message(),
                    );
                    txs.push(encode(tx));
                    generation += 1;
                }
                TxStatus::BadGeneration => {
                    if generation == 10 {
                        panic!("The first transaction will always have a valid generation");
                    } else {
                        let tx = create_tx_valid(
                            0,
                            max_priority_fee_bips,
                            admin,
                            config_value!("CHAIN_ID"),
                            encode_message(),
                        );
                        txs.push(encode(tx));
                    }
                }
                TxStatus::BadChainId => {
                    let tx = create_tx_valid(
                        generation,
                        max_priority_fee_bips,
                        admin,
                        config_value!("CHAIN_ID") + 1,
                        encode_message(),
                    );
                    txs.push(encode(tx));
                }

                TxStatus::BadSignature => {
                    let tx = create_tx_bad_sig(
                        generation,
                        max_priority_fee_bips,
                        admin,
                        config_value!("CHAIN_ID"),
                        encode_message(),
                    );
                    txs.push(encode(tx));
                }
                TxStatus::OutOfGas => {
                    let tx = create_tx_out_of_gas(
                        generation,
                        max_priority_fee_bips,
                        admin,
                        config_value!("CHAIN_ID"),
                        encode_message(),
                    );
                    txs.push(encode(tx));
                }
                TxStatus::Reverted => {
                    // A call message send by not admin will be reverted.
                    let tx = create_tx_valid(
                        0,
                        max_priority_fee_bips,
                        not_admin,
                        config_value!("CHAIN_ID"),
                        encode_message(),
                    );
                    txs.push(encode(tx));
                }
                TxStatus::BadSerialization => {
                    let tx = FullyBakedTx::new(vec![1, 2, 3]);
                    txs.push(tx);
                }
                TxStatus::SignerDoesNotExist => {
                    let tx = create_tx_bad_sender(
                        0,
                        max_priority_fee_bips,
                        config_value!("CHAIN_ID"),
                        encode_message(),
                    );
                    txs.push(encode(tx));
                }
            }
        }
        let blob = borsh::to_vec(&txs).unwrap();
        MockBlob::new_with_hash(blob, seq_da_address)
    }

    pub fn encode_message() -> IntegTestRuntimeCall<S> {
        <IntegTestRuntime<S> as EncodeCall<ValueSetter<S>>>::to_decodable(CallMessage::SetValue(8))
    }

    fn encode(tx: Transaction<IntegTestRuntime<S>, S>) -> FullyBakedTx {
        <IntegTestRuntime<S> as TransactionAuthenticator<S>>::encode_with_standard_auth(RawTx::new(
            borsh::to_vec(&tx).unwrap(),
        ))
    }
}
