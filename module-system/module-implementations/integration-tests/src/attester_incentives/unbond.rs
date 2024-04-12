use sov_attester_incentives::{CallMessage, Role, UnbondingInfo};
use sov_bank::GAS_TOKEN_ID;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::WorkingSet;
use sov_modules_stf_blueprint::TxEffect;
use sov_test_utils::attester_incentive_data::AttesterIncentivesMessageGenerator;
use sov_test_utils::runtime::TestRuntime;
use sov_test_utils::{new_test_blob_from_batch, MessageGenerator};

use super::AttesterIncentivesTestHandler;
use crate::attester_incentives::{ROLLUP_FINALITY_PERIOD, USER_BALANCE};
use crate::helpers::{Da, TestRollup, S};

#[test]
fn test_honest_unbonding() {
    // Let's do the two phase unbonding
    let mut rollup = TestRollup::new();

    let test_handler = AttesterIncentivesTestHandler::honest_attester_test_config();

    // Genesis
    let init_state_root = rollup.genesis(
        test_handler.admin_public_key,
        test_handler.sequencer_params(),
        test_handler.bank_params(),
        test_handler.attester_incentives_params(),
    );

    // Let's check that the attester is bonded
    assert_eq!(
        rollup.get_user_bond(Role::Attester, test_handler.attester_addr()),
        test_handler.attester_stake
    );

    // Let's unbond the attester.
    let attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                test_handler.attester_private_key.clone(),
                CallMessage::BeginUnbondingAttester,
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        test_handler.seq_da_addr.as_ref(),
        [3; 32],
    );

    let exec_simulation =
        rollup.execution_simulation(1, init_state_root, vec![attestation_blob.clone()], 0, None);

    assert_eq!(exec_simulation.len(), 1, "The execution simulation failed");
    let res = exec_simulation.first().unwrap();
    let new_state_root = res.state_root;

    // Let's check that the unbonding process has been initiated
    {
        assert_eq!(res.batch_receipts.len(), 1);
        let batch_receipt = res.batch_receipts.first().unwrap();
        let tx_receipt = batch_receipt.tx_receipts.first().unwrap();
        assert_eq!(tx_receipt.receipt, TxEffect::Successful);

        assert!(rollup.is_attester_unbonding(test_handler.attester_addr()));
    }

    // We now need to wait for the finality period to pass. Let's simulate it by running a few value setter transactions.
    // Then we can finish the two phase unbonding process.
    let blob = new_test_blob_from_batch(
        BatchWithId {
            txs: test_handler.value_setter.clone(),
            id: [0; 32],
        },
        test_handler.seq_da_addr.as_ref(),
        [2; 32],
    );

    let exec_simulation = rollup.execution_simulation(
        (ROLLUP_FINALITY_PERIOD).try_into().unwrap(),
        new_state_root,
        vec![blob.clone()],
        1,
        None,
    );

    for res in exec_simulation.iter() {
        assert_eq!(res.batch_receipts.len(), 1);
        let batch_receipt = res.batch_receipts.first().unwrap();
        let tx_receipt = batch_receipt.tx_receipts.first().unwrap();
        assert_eq!(tx_receipt.receipt, TxEffect::Successful);
    }

    // TODO: We need a way to sync the light clients with the current state height. Since the light clients are not implemented yet
    // we do this by hand by setting the height manually.
    let new_state_root =
        rollup.increase_and_commit_light_client_attested_height(ROLLUP_FINALITY_PERIOD);

    // Let's finish the unbonding process
    let attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                test_handler.attester_private_key.clone(),
                CallMessage::EndUnbondingAttester,
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        test_handler.seq_da_addr.as_ref(),
        [3; 32],
    );

    let exec_simulation = rollup.execution_simulation(
        1,
        new_state_root,
        vec![attestation_blob.clone()],
        (1 + ROLLUP_FINALITY_PERIOD).try_into().unwrap(),
        None,
    );

    {
        assert_eq!(exec_simulation.len(), 1, "The execution simulation failed");
        let res = exec_simulation.first().unwrap();
        assert_eq!(res.batch_receipts.len(), 1);
        let batch_receipt = res.batch_receipts.first().unwrap();
        let tx_receipt = batch_receipt.tx_receipts.first().unwrap();
        assert_eq!(tx_receipt.receipt, TxEffect::Successful);

        let mut working_set = WorkingSet::<S>::new(rollup.storage());

        assert!(!rollup.is_attester_unbonding(test_handler.attester_addr()));

        assert_eq!(
            rollup.get_user_bond(Role::Attester, test_handler.attester_addr()),
            0
        );

        // We have to check that the attester has received the stake amount back
        // We have to substract 2 * gas_per_transaction because the attester has to pay for the gas
        // for both the start and end unbonding messages
        assert_eq!(
            rollup.bank().get_balance_of(
                &test_handler.attester_addr(),
                GAS_TOKEN_ID,
                &mut working_set
            ),
            Some(USER_BALANCE - 2 * rollup.gas_per_transaction())
        );
    }
}

// We cannot unbond an attester that has not been bonded.
#[test]
fn test_unbonding_without_bonded() {
    // Let's do the two phase unbonding
    let mut rollup = TestRollup::new();

    let test_handle = AttesterIncentivesTestHandler::honest_attester_test_config();

    // Genesis
    let init_state_root = rollup.genesis(
        test_handle.admin_public_key,
        test_handle.sequencer_params(),
        test_handle.bank_params(),
        test_handle.attester_incentives_params(),
    );

    // Let's finish the unbonding process
    let attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                test_handle.attester_private_key.clone(),
                CallMessage::EndUnbondingAttester,
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        test_handle.seq_da_addr.as_ref(),
        [3; 32],
    );

    let exec_simulation =
        rollup.execution_simulation(1, init_state_root, vec![attestation_blob.clone()], 1, None);

    // The transaction needs to revert
    {
        assert_eq!(exec_simulation.len(), 1, "The execution simulation failed");
        let res = exec_simulation.first().unwrap();
        assert_eq!(res.batch_receipts.len(), 1);
        let batch_receipt = res.batch_receipts.first().unwrap();
        let tx_receipt = batch_receipt.tx_receipts.first().unwrap();
        assert_eq!(tx_receipt.receipt, TxEffect::Reverted);
    }
}

// We cannot unbond an attester before the finality period has passed.
#[test]
fn test_premature_unbonding() {
    // Let's do the two phase unbonding
    let mut rollup = TestRollup::new();

    let test_handle = AttesterIncentivesTestHandler::honest_attester_test_config();

    // Genesis
    let init_state_root = rollup.genesis(
        test_handle.admin_public_key,
        test_handle.sequencer_params(),
        test_handle.bank_params(),
        test_handle.attester_incentives_params(),
    );

    // Let's check that the attester is bonded
    assert_eq!(
        rollup.get_user_bond(Role::Attester, test_handle.attester_addr()),
        test_handle.attester_stake
    );

    // Let's unbond the attester.
    let attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                test_handle.attester_private_key.clone(),
                CallMessage::BeginUnbondingAttester,
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        test_handle.seq_da_addr.as_ref(),
        [3; 32],
    );

    let exec_simulation =
        rollup.execution_simulation(1, init_state_root, vec![attestation_blob.clone()], 0, None);

    assert_eq!(exec_simulation.len(), 1, "The execution simulation failed");
    let res = exec_simulation.first().unwrap();
    let new_state_root = res.state_root;

    // Let's check that the unbonding process has been initiated
    {
        assert_eq!(res.batch_receipts.len(), 1);
        let batch_receipt = res.batch_receipts.first().unwrap();
        let tx_receipt = batch_receipt.tx_receipts.first().unwrap();
        assert_eq!(tx_receipt.receipt, TxEffect::Successful);

        let mut working_set = WorkingSet::<S>::new(rollup.storage());

        let unbonding_info = rollup
            .attester_incentives()
            .unbonding_attesters
            .get(&test_handle.attester_addr(), &mut working_set)
            .expect("The attester should be unbonding");

        assert_eq!(
            unbonding_info,
            UnbondingInfo {
                unbonding_initiated_height: 0,
                amount: test_handle.attester_stake
            }
        );
    }

    // Let's finish the unbonding process without waiting for the finality period to pass.
    let attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                test_handle.attester_private_key.clone(),
                CallMessage::EndUnbondingAttester,
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        test_handle.seq_da_addr.as_ref(),
        [3; 32],
    );

    let exec_simulation =
        rollup.execution_simulation(1, new_state_root, vec![attestation_blob.clone()], 1, None);

    // This is not a slashable offense, so the transaction should revert
    {
        assert_eq!(exec_simulation.len(), 1, "The execution simulation failed");
        let res = exec_simulation.first().unwrap();
        assert_eq!(res.batch_receipts.len(), 1);
        let batch_receipt = res.batch_receipts.first().unwrap();
        let tx_receipt = batch_receipt.tx_receipts.first().unwrap();
        assert_eq!(tx_receipt.receipt, TxEffect::Reverted);
    }
}
