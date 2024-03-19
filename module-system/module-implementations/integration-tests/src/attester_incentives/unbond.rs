use std::rc::Rc;

use sov_attester_incentives::{CallMessage, Role, UnbondingInfo};
use sov_bank::get_genesis_token_address;
use sov_mock_da::MockValidityCondChecker;
use sov_mock_zkvm::MockCodeCommitment;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::{PrivateKey, WorkingSet};
use sov_modules_stf_blueprint::TxEffect;
use sov_test_utils::attester_incentive_data::AttesterIncentivesMessageGenerator;
use sov_test_utils::value_setter_data::ValueSetterMessages;
use sov_test_utils::{new_test_blob_from_batch, MessageGenerator, TestPrivateKey};

use crate::helpers::{
    AttesterIncentivesParams, BankParams, Da, SequencerParams, TestRollup, TestRuntime, S,
};

#[test]
fn test_honest_unbonding() {
    // Let's do the two phase unbonding
    let mut rollup = TestRollup::new();

    let value_setter_messages = ValueSetterMessages::prepopulated();
    let value_setter = value_setter_messages.create_raw_txs::<TestRuntime<S, Da>>();

    let admin_pub_key = value_setter_messages.messages[0].admin.to_address();

    // An attester that is already bonded at genesis
    let honest_attester_pkey = TestPrivateKey::generate();
    let honest_attester_addr = honest_attester_pkey.to_address();
    let honest_attester_stake = 100;

    let seq_params = SequencerParams::default();
    let seq_da_addr = seq_params.da_address;
    let bank_params = BankParams::default();
    let token_addr =
        get_genesis_token_address::<S>(bank_params.token_name.as_str(), bank_params.salt);

    let rollup_finality_period = 2;

    let attester_params = AttesterIncentivesParams {
        initial_attesters: vec![(honest_attester_addr, honest_attester_stake)],
        reward_token_supply_address: [1; 32].into(),
        rollup_finality_period,
        minimum_attester_bond: 100,
        minimum_challenger_bond: 100,
        maximum_attested_height: 0,
        light_client_finalized_height: 0,
        commitment_to_allowed_challenge_method: MockCodeCommitment([0; 32]),
        validity_condition_checker: MockValidityCondChecker::default(),
    };

    // Genesis
    let init_state_root = rollup.genesis(admin_pub_key, seq_params, bank_params, attester_params);

    // Let's check that the attester is bonded
    assert_eq!(
        rollup.get_user_bond(Role::Attester, honest_attester_addr),
        honest_attester_stake
    );

    // Let's unbond the attester.
    let attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                honest_attester_pkey.clone(),
                CallMessage::BeginUnbondingAttester,
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        seq_da_addr.as_ref(),
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

        assert!(rollup.is_attester_unbonding(honest_attester_addr));
    }

    // We now need to wait for the finality period to pass. Let's simulate it by running a few value setter transactions.
    // Then we can finish the two phase unbonding process.
    let blob = new_test_blob_from_batch(
        BatchWithId {
            txs: value_setter,
            id: [0; 32],
        },
        seq_da_addr.as_ref(),
        [2; 32],
    );

    let exec_simulation = rollup.execution_simulation(
        (rollup_finality_period).try_into().unwrap(),
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
        rollup.increase_and_commit_light_client_attested_height(rollup_finality_period);

    // Let's finish the unbonding process
    let attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                honest_attester_pkey.clone(),
                CallMessage::EndUnbondingAttester,
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        seq_da_addr.as_ref(),
        [3; 32],
    );

    let exec_simulation = rollup.execution_simulation(
        1,
        new_state_root,
        vec![attestation_blob.clone()],
        (1 + rollup_finality_period).try_into().unwrap(),
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

        assert!(!rollup.is_attester_unbonding(honest_attester_addr));

        assert_eq!(
            rollup.get_user_bond(Role::Attester, honest_attester_addr),
            0
        );

        // We have to check that the attester has received the stake amount back
        assert_eq!(
            rollup
                .bank()
                .get_balance_of(honest_attester_addr, token_addr, &mut working_set),
            Some(honest_attester_stake)
        );
    }
}

// We cannot unbond an attester that has not been bonded.
#[test]
fn test_unbonding_without_bonded() {
    // Let's do the two phase unbonding
    let mut rollup = TestRollup::new();

    let value_setter_messages: ValueSetterMessages<S> = ValueSetterMessages::prepopulated();

    let admin_private_key: Rc<TestPrivateKey> = value_setter_messages.messages[0].admin.clone();
    let admin_pub_key = admin_private_key.to_address();

    // An attester that is already bonded at genesis
    let attester_pkey = TestPrivateKey::generate();

    let seq_params = SequencerParams::default();
    let seq_da_addr = seq_params.da_address;
    let bank_params = BankParams::default();
    let attester_params = AttesterIncentivesParams::default();

    // Genesis
    let init_state_root = rollup.genesis(admin_pub_key, seq_params, bank_params, attester_params);

    // Let's finish the unbonding process
    let attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                attester_pkey.clone(),
                CallMessage::EndUnbondingAttester,
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        seq_da_addr.as_ref(),
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

    let value_setter_messages: ValueSetterMessages<S> = ValueSetterMessages::prepopulated();

    let admin_private_key: Rc<TestPrivateKey> = value_setter_messages.messages[0].admin.clone();
    let admin_pub_key = admin_private_key.to_address();

    // An attester that is already bonded at genesis
    let honest_attester_pkey = TestPrivateKey::generate();
    let honest_attester_addr = honest_attester_pkey.to_address();
    let honest_attester_stake = 100;

    let seq_params = SequencerParams::default();
    let seq_da_addr = seq_params.da_address;
    let bank_params = BankParams::default();

    let rollup_finality_period = 2;

    let attester_params = AttesterIncentivesParams {
        initial_attesters: vec![(honest_attester_addr, honest_attester_stake)],
        reward_token_supply_address: [1; 32].into(),
        rollup_finality_period,
        minimum_attester_bond: 100,
        minimum_challenger_bond: 100,
        maximum_attested_height: 0,
        light_client_finalized_height: 0,
        commitment_to_allowed_challenge_method: MockCodeCommitment([0; 32]),
        validity_condition_checker: MockValidityCondChecker::default(),
    };

    // Genesis
    let init_state_root = rollup.genesis(admin_pub_key, seq_params, bank_params, attester_params);

    // Let's check that the attester is bonded
    assert_eq!(
        rollup.get_user_bond(Role::Attester, honest_attester_addr),
        honest_attester_stake
    );

    // Let's unbond the attester.
    let attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                honest_attester_pkey.clone(),
                CallMessage::BeginUnbondingAttester,
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        seq_da_addr.as_ref(),
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
            .get(&honest_attester_addr, &mut working_set)
            .expect("The attester should be unbonding");

        assert_eq!(
            unbonding_info,
            UnbondingInfo {
                unbonding_initiated_height: 0,
                amount: honest_attester_stake
            }
        );
    }

    // Let's finish the unbonding process without waiting for the finality period to pass.
    let attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                honest_attester_pkey.clone(),
                CallMessage::EndUnbondingAttester,
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        seq_da_addr.as_ref(),
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
