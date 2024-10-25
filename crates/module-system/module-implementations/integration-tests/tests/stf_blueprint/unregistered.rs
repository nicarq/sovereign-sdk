use std::env;

use helpers::*;
use serial_test::serial;
use sov_attester_incentives::AttesterIncentives;
use sov_bank::IntoPayable;
use sov_mock_da::{MockAddress, MockBlob};
use sov_modules_api::macros::config_value;
use sov_modules_api::transaction::{
    PriorityFeeBips, SequencerReward, Transaction, UnsignedTransaction,
};
use sov_modules_api::{ApiStateAccessor, DaSpec, Gas, ModuleInfo, RawTx, Spec, TxEffect};
use sov_rollup_interface::da::RelevantBlobs;
use sov_sequencer_registry::SequencerRegistry;
use sov_test_utils::{EncodeCall, TestUser};

use super::{get_balance, get_seq_bond, setup, TxStatus};
use crate::stf_blueprint::IntegTestRuntime;
type S = sov_test_utils::TestSpec;

const BOND_AMOUNT: u64 = 100;

fn check_unreg_txs(tx_statuses: Vec<TxStatus>, priority_fee_bips: PriorityFeeBips) {
    let (mut runner, users, _) = setup(tx_statuses.len());

    // Every potential sequencer gets a unique DA address.
    let mut potential_sequencers_with_statuses = Vec::new();
    for (i, status) in tx_statuses.into_iter().enumerate() {
        let da_address = MockAddress::new([i as u8; 32]);
        let potential_seq = PotentialSequencer {
            user: users[i].clone(),
            da_address,
        };

        potential_sequencers_with_statuses.push((status, potential_seq));
    }

    let blobs_with_pot_sequencers =
        create_blobs_from_unregistered_seq(priority_fee_bips, potential_sequencers_with_statuses);

    for (blob, potential_seq) in blobs_with_pot_sequencers {
        let start = runner.query_state(|state| potential_seq.balances(state));

        let unregistered_blobs = RelevantBlobs {
            proof_blobs: Default::default(),
            batch_blobs: vec![blob],
        };
        let result =
            runner.execute::<RelevantBlobs<MockBlob>, SequencerRegistry<S>>(unregistered_blobs);

        let batch_receipt = &result.batch_receipts[0];
        let gas_price = &batch_receipt.gas_price;

        let tx_receipt = &batch_receipt.tx_receipts[0];

        let gas_value_charged_to_user;
        let seq_fee;
        let bond_amount;
        match &tx_receipt.receipt {
            TxEffect::Successful(tx_contents) => {
                let gas_value = tx_contents.gas_used.value(gas_price);
                gas_value_charged_to_user = gas_value;
                seq_fee = priority_fee_bips.apply(gas_value).unwrap();
                bond_amount = BOND_AMOUNT;
            }
            TxEffect::Skipped(_tx_contents) => {
                // The sequencer is not bonded so we can't penalize them for skipped transactions.
                // In this case no one is charged for the failed transaction.
                gas_value_charged_to_user = 0;
                seq_fee = 0;
                bond_amount = 0;
            }
            TxEffect::Reverted(_tx_contents) => {
                todo!()
            }
        }

        let end = runner.query_state(|state| potential_seq.balances(state));

        // Sequencer fees are transferred to the bond in the sequencer registry.
        assert_eq!(end.potential_seq_bond, seq_fee + bond_amount);
        // The `seq_fee`` is redundant here, but I am leaving it as documentation to explain what is happening.
        // The user acts as a sequencer, transferring the fee from their bank balance to the bond in the sequencer registry.
        assert_eq!(
            end.potential_seq_bank_balance + end.potential_seq_bond - seq_fee,
            start.potential_seq_bank_balance - gas_value_charged_to_user - seq_fee
        );

        assert_eq!(
            end.attester_module_balance,
            start.attester_module_balance + gas_value_charged_to_user
        );

        assert_eq!(end.total_balance(), start.total_balance());

        assert_eq!(
            batch_receipt.inner.outcome,
            // TODO account for batch_hook_gas_value
            sov_modules_api::BatchSequencerOutcome::Rewarded(SequencerReward(seq_fee))
        );
    }
}

// Execute batch of valid registrations.
#[test]
#[serial]
fn execute_seq_registration_success_test() {
    env::set_var("SOV_SDK_CONST_OVERRIDE_BATCH_HOOK_GAS", "[0, 0]");
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![TxStatus::Success, TxStatus::Success];
    check_unreg_txs(tx_statuses, priority_fee_bips);
}

// Execute batch of invalid registrations.
#[test]
#[serial]
fn execute_seq_registration_failure_test() {
    env::set_var("SOV_SDK_CONST_OVERRIDE_BATCH_HOOK_GAS", "[0, 0]");
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![TxStatus::BadNonce, TxStatus::BadNonce];
    check_unreg_txs(tx_statuses, priority_fee_bips);
}

mod helpers {
    use super::*;
    // A user that is not a registered sequencer and attempts to register itself as one.
    pub(crate) struct PotentialSequencer {
        pub(crate) user: TestUser<S>,
        pub(crate) da_address: MockAddress,
    }

    impl PotentialSequencer {
        pub(crate) fn balances(&self, state: &mut ApiStateAccessor<S>) -> Balances {
            let attester_module = AttesterIncentives::<S>::default();
            Balances {
                potential_seq_bank_balance: get_balance(&self.user.address(), state),
                attester_module_balance: get_balance(attester_module.id().to_payable(), state),
                potential_seq_bond: get_seq_bond(&self.da_address, state).unwrap_or(0),
            }
        }
    }

    #[derive(Debug, Eq, PartialEq)]
    pub(crate) struct Balances {
        pub(crate) potential_seq_bank_balance: u64,
        pub(crate) potential_seq_bond: u64,
        pub(crate) attester_module_balance: u64,
    }

    impl Balances {
        pub(crate) fn total_balance(&self) -> u64 {
            self.potential_seq_bank_balance + self.potential_seq_bond + self.attester_module_balance
        }
    }

    fn create_tx_blob_valid(
        nonce: u64,
        max_priority_fee_bips: PriorityFeeBips,
        signer: &TestUser<S>,
        da_address: <<S as Spec>::Da as DaSpec>::Address,
    ) -> MockBlob {
        let encoded_message =
            <IntegTestRuntime<S> as EncodeCall<SequencerRegistry<S>>>::encode_call(
                sov_sequencer_registry::CallMessage::Register {
                    da_address,
                    amount: BOND_AMOUNT,
                },
            );

        let utx = UnsignedTransaction::new(
            encoded_message.clone(),
            config_value!("CHAIN_ID"),
            max_priority_fee_bips,
            200_000,
            nonce,
            None,
        );

        let transaction = Transaction::<S>::new_signed_tx(signer.private_key(), utx);

        let tx_data = borsh::to_vec(&transaction).unwrap();
        let raw_tx = RawTx { data: tx_data };
        let tx_blob = borsh::to_vec(&raw_tx).unwrap();

        MockBlob::new_with_hash(tx_blob, da_address)
    }

    pub(crate) fn create_blobs_from_unregistered_seq(
        max_priority_fee_bips: PriorityFeeBips,
        potential_seqs_with_statuses: Vec<(TxStatus, PotentialSequencer)>,
    ) -> Vec<(MockBlob, PotentialSequencer)> {
        let mut blobs = Vec::new();

        for (status, pot_seq) in potential_seqs_with_statuses.into_iter() {
            let blob = create_blob(&status, max_priority_fee_bips, &pot_seq);
            blobs.push((blob, pot_seq));
        }

        blobs
    }

    pub(crate) fn create_blob(
        status: &TxStatus,
        max_priority_fee_bips: PriorityFeeBips,
        potential_seq: &PotentialSequencer,
    ) -> MockBlob {
        match status {
            TxStatus::Success => create_tx_blob_valid(
                0,
                max_priority_fee_bips,
                &potential_seq.user,
                potential_seq.da_address,
            ),
            TxStatus::BadNonce => create_tx_blob_valid(
                999,
                max_priority_fee_bips,
                &potential_seq.user,
                potential_seq.da_address,
            ),
            TxStatus::BadChainId => todo!(),
            TxStatus::BadSignature => todo!(),
            TxStatus::Reverted => todo!(),
        }
    }
}
