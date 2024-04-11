use sov_bank::{Bank, IntoPayable, ReserveGasError, GAS_TOKEN_ID};
use sov_modules_api::transaction::{PriorityFeeBips, Transaction};
use sov_modules_api::{GasMeter, GasUnit, ModuleInfo, PrivateKey, Spec, StateCheckpoint};
use sov_state::{DefaultStorageSpec, ProverStorage};
use sov_test_utils::{simple_bank_setup, TestPrivateKey};
type S = sov_test_utils::TestSpec;
use sov_modules_api::Gas;
mod helpers;
use sov_modules_api::GasArray;

pub type Storage = ProverStorage<DefaultStorageSpec>;

const DEFAULT_CHAIN_ID: u64 = 0;

fn generate_empty_tx(
    max_priority_fee: PriorityFeeBips,
    max_fee: u64,
    gas_limit: Option<GasUnit<2>>,
) -> Transaction<S> {
    Transaction::new_signed_tx(
        &TestPrivateKey::generate(),
        vec![],
        DEFAULT_CHAIN_ID,
        max_priority_fee,
        max_fee,
        gas_limit,
        0,
    )
}

/// Helper struct that gets instantiated following the `reserve_gas_helper` method. Contains useful test parameters.
struct CapabilityTestParams {
    pub bank: Bank<S>,
    pub transaction: Transaction<S>,
    pub gas_meter: GasMeter<<S as Spec>::Gas>,
    pub sender_address: <S as Spec>::Address,
    pub state_checkpoint: StateCheckpoint<S>,
}

/// Helper function that creates a simple bank setup (one account with `initial_balance`), generates a transaction
/// with the given gas parameters, reserves some gas, checks the resulting gas meter, and returns useful test parameters.
fn reserve_gas_helper(
    initial_balance: u64,
    max_priority_fee: PriorityFeeBips,
    gas_limit: Option<GasUnit<2>>,
    gas_price: &<<S as Spec>::Gas as Gas>::Price,
) -> CapabilityTestParams {
    let (sender_address, bank, mut checkpoint) = simple_bank_setup(initial_balance);

    let transaction: Transaction<S> =
        generate_empty_tx(max_priority_fee, initial_balance, gas_limit.clone());

    // We try to reserve gas, this should succeed because we have enough balance.
    let gas_meter = bank
        .reserve_gas(&transaction, gas_price, &sender_address, &mut checkpoint)
        .expect("The reserve gas operation should not fail");

    let expected_balance_reserved = match gas_limit {
        Some(gas_limit) => gas_limit.value(gas_price),
        None => initial_balance,
    };

    assert_eq!(
        gas_meter,
        GasMeter::new(expected_balance_reserved, gas_price.clone())
    );

    CapabilityTestParams {
        bank,
        transaction,
        gas_meter,
        sender_address,
        state_checkpoint: checkpoint,
    }
}

/// Tests the happy path of the `reserve_gas` method. We try to reserve gas, then consume it and refund it.
/// The priority fee is zero.
#[test]
fn test_honest_reserve_gas_capability_without_priority_fee() {
    let initial_balance = 100;
    let mut params = reserve_gas_helper(
        initial_balance,
        PriorityFeeBips::ZERO,
        Some(GasUnit::from_slice(&[initial_balance / 2; 2])),
        &<<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]),
    );

    // Let's consume the all the gas, this should succeed because there is enough gas left in the meter.
    params
        .gas_meter
        .charge_gas(&GasUnit::from_slice(&[initial_balance / 2; 2]))
        .expect("The charge gas operation should not fail");

    // We try to refund the gas, this should never fail. The gas is already consumed so the sender balance should be zero.
    params.bank.refund_remaining_gas(
        &params.transaction,
        &params.gas_meter,
        &params.sender_address,
        &params.bank.id().to_payable(),
        &params.bank.id().to_payable(),
        &mut params.state_checkpoint,
    );

    assert_eq!(
        params
            .bank
            .get_balance_of(
                &params.sender_address,
                GAS_TOKEN_ID,
                &mut params.state_checkpoint
            )
            .expect("The sender balance should exist"),
        0
    );
}

/// Tests the happy path of the `reserve_gas` method. We try to reserve gas, then consume it and refund it.
/// The priority fee is non zero but the difference between the max fee and the maximum gas value is zero
/// hence the priority fee is not charged.
#[test]
fn test_honest_reserve_gas_capability_does_not_charge_priority_fee() {
    let initial_balance = 100;
    let mut params = reserve_gas_helper(
        initial_balance,
        PriorityFeeBips::from_percentage(10),
        Some(GasUnit::from_slice(&[initial_balance / 2; 2])),
        &<<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]),
    );

    // Let's consume the all the gas, this should succeed because there is enough gas left in the meter.
    params
        .gas_meter
        .charge_gas(&GasUnit::from_slice(&[initial_balance / 2; 2]))
        .expect("The charge gas operation should not fail");

    // We try to refund the gas, this should never fail. The gas is already consumed so the sender balance should be zero.
    params.bank.refund_remaining_gas(
        &params.transaction,
        &params.gas_meter,
        &params.sender_address,
        &params.bank.id().to_payable(),
        &params.bank.id().to_payable(),
        &mut params.state_checkpoint,
    );

    assert_eq!(
        params
            .bank
            .get_balance_of(
                &params.sender_address,
                GAS_TOKEN_ID,
                &mut params.state_checkpoint
            )
            .expect("The sender balance should exist"),
        0
    );
}

/// Tests the happy path of the `reserve_gas` method. We try to reserve gas, then consume it and refund it.
/// The priority fee is non zero and is charged as part of the transaction.
#[test]
fn test_honest_reserve_gas_capability_with_priority_fee() {
    let initial_balance = 100;
    let max_priority_fee = PriorityFeeBips::from_percentage(10);
    let gas_price = &<<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]);

    let mut params = reserve_gas_helper(
        initial_balance,
        PriorityFeeBips::from_percentage(10),
        Some(GasUnit::from_slice(&[initial_balance / 4; 2])),
        &<<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]),
    );

    let gas_to_charge = GasUnit::from_slice(&[initial_balance / 20; 2]);

    // We try to charge for some of the gas
    params
        .gas_meter
        .charge_gas(&gas_to_charge)
        .expect("The charge gas operation should not fail");

    // We try to refund the gas, this should never fail.
    params.bank.refund_remaining_gas(
        &params.transaction,
        &params.gas_meter,
        &params.sender_address,
        &params.bank.id().to_payable(),
        &params.bank.id().to_payable(),
        &mut params.state_checkpoint,
    );

    // The sender should have been refunded:
    // initial_balance - gas_to_charge_value * (1 + max_priority_fee_percentage)
    let gas_to_charge_value = gas_to_charge.value(gas_price);

    let refund_amount = initial_balance
        - gas_to_charge_value
        - max_priority_fee
            .apply(gas_to_charge_value)
            .expect("This should not overflow");

    assert_eq!(
        params
            .bank
            .get_balance_of(
                &params.sender_address,
                GAS_TOKEN_ID,
                &mut params.state_checkpoint
            )
            .expect("The sender balance should exist"),
        refund_amount
    );

    // We now test that the bank has locked the remaining amount
    assert_eq!(
        params
            .bank
            .get_balance_of(
                params.bank.id().to_payable(),
                GAS_TOKEN_ID,
                &mut params.state_checkpoint
            )
            .expect("The bank balance should exist"),
        initial_balance - refund_amount
    );
}

/// Tests that the `reserve_gas` method fails if the sender balance is not high enough to pay for the gas.
#[test]
fn test_reserve_gas_not_enough_balance() {
    let initial_balance = 100;
    let (sender_address, bank, mut checkpoint) = simple_bank_setup(initial_balance);

    let gas_price = <<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]);

    // This transaction has a maximum fee of twice the initial balance.
    let transaction: Transaction<S> =
        generate_empty_tx(PriorityFeeBips::ZERO, 2 * initial_balance, None);

    // We try to reserve gas, this should fail because we have not enough balance.
    let reserve_gas_result = bank
        .reserve_gas(&transaction, &gas_price, &sender_address, &mut checkpoint)
        .expect_err("The reserve gas operation should fail");

    assert_eq!(
        reserve_gas_result,
        ReserveGasError::InsufficientBalanceToReserveGas
    );
}

/// Tests that the `reserve_gas` method fails if the current gas price is too high to cover the maximum fee for the transaction.
/// This check is only performed if the `gas_limit` is set.
#[test]
fn test_reserve_gas_price_too_high() {
    let initial_balance = 100;
    let (sender_address, bank, mut checkpoint) = simple_bank_setup(initial_balance);

    // This transaction has gas limit set to [50; 2], which means the associated gas price is [1; 2].
    let transaction: Transaction<S> = generate_empty_tx(
        PriorityFeeBips::ZERO,
        initial_balance,
        Some(GasUnit::from_slice(&[50; 2])),
    );

    // The gas price is [2; 2] which is higher than the one associated with the gas limit.
    let gas_price = <<S as Spec>::Gas as Gas>::Price::from_slice(&[2; 2]);

    // We try to reserve gas, this should fail because the gas price is too high.
    let reserve_gas_result = bank
        .reserve_gas(&transaction, &gas_price, &sender_address, &mut checkpoint)
        .expect_err("The reserve gas operation should fail");

    assert_eq!(reserve_gas_result, ReserveGasError::CurrentGasPriceTooHigh);
}

/// Tests that the `reserve_gas` method does not overflow or panic if the total gas amount to reserve is `u64::MAX`.
#[test]
fn test_reserve_gas_should_not_overflow_or_panic_zero_priority() {
    let initial_balance = u64::MAX;
    let mut params = reserve_gas_helper(
        initial_balance,
        PriorityFeeBips::from_percentage(0),
        None,
        &<<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]),
    );

    // We try to refund the gas, this should never fail. The gas is already consumed so the sender balance should be zero.
    params.bank.refund_remaining_gas(
        &params.transaction,
        &params.gas_meter,
        &params.sender_address,
        &params.bank.id().to_payable(),
        &params.bank.id().to_payable(),
        &mut params.state_checkpoint,
    );

    assert_eq!(
        params
            .bank
            .get_balance_of(
                &params.sender_address,
                GAS_TOKEN_ID,
                &mut params.state_checkpoint
            )
            .expect("The sender balance should exist"),
        initial_balance
    );
}

/// Tests that the `reserve_gas` method does not overflow or panic if the total gas amount to reserve is `u64::MAX` and the priority fee is not zero.
#[test]
fn test_reserve_gas_should_not_overflow_or_panic_non_zero_priority() {
    let initial_balance = u64::MAX;
    let mut params = reserve_gas_helper(
        initial_balance,
        PriorityFeeBips::from_percentage(10),
        None,
        &<<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]),
    );

    // We try to refund the gas, this should never fail. The gas is already consumed so the sender balance should be zero.
    params.bank.refund_remaining_gas(
        &params.transaction,
        &params.gas_meter,
        &params.sender_address,
        &params.bank.id().to_payable(),
        &params.bank.id().to_payable(),
        &mut params.state_checkpoint,
    );

    assert_eq!(
        params
            .bank
            .get_balance_of(
                &params.sender_address,
                GAS_TOKEN_ID,
                &mut params.state_checkpoint
            )
            .expect("The sender balance should exist"),
        initial_balance
    );
}
