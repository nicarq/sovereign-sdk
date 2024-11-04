use sov_mock_zkvm::MockZkvm;
use sov_rollup_interface::execution_mode::Native;
use sov_test_utils::MockDaSpec;

use crate::default_spec::DefaultSpec;
use crate::transaction::{
    transaction_consumption_helper, PriorityFeeBips, SequencerReward, TransactionConsumption,
};
use crate::{GasPrice, GasUnit};

/// Consume all the remaining gas, so the transaction reward is the same as the base fee and there is no priority fee.
#[test]
fn test_compute_transaction_reward_consume_all_gas() {
    const REMAINING_FUNDS: u64 = 100;

    let tx_reward =
        transaction_consumption_helper::<DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, Native>>(
            &GasUnit::from([REMAINING_FUNDS / 2; 2]),
            &GasPrice::from([1; 2]),
            REMAINING_FUNDS,
            PriorityFeeBips::from_percentage(10),
        );

    assert_eq!(
        tx_reward,
        TransactionConsumption {
            remaining_funds: 0,
            base_fee: GasUnit::from([REMAINING_FUNDS / 2; 2]),
            priority_fee: 0,
            gas_price: GasPrice::from([1; 2])
        }
    );
}

/// Consume half of the remaining gas, so the transaction reward is half of the initial funds and there is a maximum priority fee (100%).
#[test]
fn test_compute_transaction_reward_consume_not_all_gas() {
    const REMAINING_FUNDS: u64 = 100;

    let tx_reward =
        transaction_consumption_helper::<DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, Native>>(
            &GasUnit::from([REMAINING_FUNDS / 4; 2]),
            &GasPrice::from([1; 2]),
            REMAINING_FUNDS,
            PriorityFeeBips::from_percentage(100),
        );

    assert_eq!(
        tx_reward,
        TransactionConsumption {
            remaining_funds: 0,
            base_fee: GasUnit::from([REMAINING_FUNDS / 4; 2]),
            priority_fee: 50,
            gas_price: GasPrice::from([1; 2])
        }
    );
}

#[test]
fn test_display_transaction_reward() {
    let tx_reward = TransactionConsumption::<GasUnit<2>> {
        remaining_funds: 10,
        base_fee: GasUnit::from([100; 2]),
        priority_fee: 50,
        gas_price: GasPrice::from([1; 2]),
    };

    assert_eq!(
        format!("{}", tx_reward),
        "TransactionConsumption { remaining_funds: 10, base_fee: GasUnit[100, 100], priority_fee: 50, gas_price: GasPrice[1, 1] }"
    );
}

#[test]
fn test_display_sequencer_reward() {
    assert_eq!(SequencerReward(100).to_string(), "SequencerReward(100)");
}
