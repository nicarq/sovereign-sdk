use borsh::BorshSerialize;
use sov_bank::{IntoPayable, Payable};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::ApplyBatchHooks;
use sov_modules_api::runtime::capabilities::RawTx;
use sov_modules_api::transaction::PriorityFeeBips;
use sov_modules_api::{Gas, GasArray, GasMeter, GasUnit, ModuleInfo, Spec};
use sov_test_utils::generate_empty_tx;

use super::helpers::{TestSequencer, INITIAL_BALANCE, S};
use crate::tests::helpers::INITIAL_BALANCE_LARGE;
use crate::SequencerOutcome;

/// Tests that the sequencer gets correctly rewarded when it processes a batch and:
/// - the `GasEnforcer` capability is correctly used (hence the module has enough funds to pay for the reward)
/// - the `end_batch_hook` is called with a `SequencerOutcome::Rewarded` result
#[test]
fn test_reward_sequencer() {
    // Genesis initialization.
    // We need to pass the large balance to make sure we have enough funds to pay for the tip and the sequencer registration
    let (sequencer_test, mut working_set) =
        TestSequencer::initialize_test(INITIAL_BALANCE_LARGE, false);
    let balance_after_genesis = sequencer_test
        .query_sequencer_balance(&mut working_set)
        .unwrap();
    let registry_balance_after_genesis = sequencer_test
        .query_balance(sequencer_test.registry.id().to_payable(), &mut working_set)
        .unwrap();

    let seq_address = &sequencer_test.sequencer_config.seq_rollup_address;
    let seq_da_address = sequencer_test.sequencer_config.seq_da_address;
    let seq_address_as_token_holder = seq_address.as_token_holder();

    let gas_price = <<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]);

    let tx = generate_empty_tx(
        PriorityFeeBips::from_percentage(10),
        balance_after_genesis,
        None,
    );

    let mut checkpoint = working_set.checkpoint().0;

    // Execute the begin batch hook
    let mut batch_test = BatchWithId {
        txs: vec![RawTx {
            data: tx.try_to_vec().unwrap(),
        }],
        id: [0; 32],
    };

    sequencer_test
        .registry
        .begin_batch_hook(&mut batch_test, &seq_da_address, &mut checkpoint)
        .expect("The begin batch hook should succeed");

    let tx = tx.into();
    // Reserves some gas for the bank
    let mut gas_meter = sequencer_test
        .bank
        .reserve_gas(&tx, &gas_price, seq_address, &mut checkpoint)
        .expect(
            "
        The gas reserve should not fail",
        );

    // Charges the gas
    gas_meter
        .charge_gas(&GasUnit::from_slice(&[balance_after_genesis / 4; 2]))
        .expect("The charge gas operation should not fail");

    // We refund the base tip to the sequencer account and send the tip to the registry
    sequencer_test.bank.refund_remaining_gas(
        &tx,
        &gas_meter,
        seq_address,
        &seq_address_as_token_holder,
        &sequencer_test.registry.id().to_payable(),
        &mut checkpoint,
    );

    let registry_balance_after_refund = sequencer_test
        .query_balance(sequencer_test.registry.id().to_payable(), &mut checkpoint)
        .unwrap();

    assert_ne!(
        registry_balance_after_genesis, registry_balance_after_refund,
        "The tip has not been refunded to the sequencer registry"
    );

    // We refund the tip to the sequencer account in the end batch hook
    sequencer_test.registry.end_batch_hook(
        SequencerOutcome::Rewarded(registry_balance_after_refund - registry_balance_after_genesis),
        &seq_da_address,
        &mut checkpoint,
    );

    // The sequencer balance should be the same as the initial balance after the refunds
    assert_eq!(
        sequencer_test
            .query_sequencer_balance(&mut checkpoint)
            .unwrap(),
        balance_after_genesis
    );
}

/// Tests that the sequencer gets correctly penalized when it incorrectly processes a batch
#[test]
fn test_penalize_sequencer() {
    // Genesis initialization.
    let (sequencer_test, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);
    let seq_da_address = sequencer_test.sequencer_config.seq_da_address;

    let seq_stake_after_genesis = sequencer_test
        .query_sender_balance(&seq_da_address, &mut working_set)
        .unwrap();

    let mut state_checkpoint = working_set.checkpoint().0;

    // We penalize the sequencer by removing all its stake
    sequencer_test.registry.end_batch_hook(
        SequencerOutcome::Penalized(seq_stake_after_genesis),
        &seq_da_address,
        &mut state_checkpoint,
    );

    // The sequencer stake should be zero
    let mut working_set = state_checkpoint.to_revertable(GasMeter::unmetered());
    assert_eq!(
        sequencer_test
            .query_sender_balance(&seq_da_address, &mut working_set)
            .unwrap(),
        0
    );
}
