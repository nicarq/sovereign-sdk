use sov_bank::{Bank, IntoPayable, ReserveGasError, GAS_TOKEN_ID};
use sov_modules_api::transaction::{
    AuthenticatedTransactionData, PriorityFeeBips, Transaction, TxGasMeter,
};
use sov_modules_api::{
    Address, Gas, GasMeter, GasUnit, ModuleInfo, Spec, StateCheckpoint, UnlimitedGasMeter,
};
use sov_test_utils::{generate_empty_tx, simple_bank_setup};
mod helpers;
use sov_modules_api::GasArray;

type S = sov_test_utils::TestSpec;

/// Helper struct that gets instantiated following the `reserve_gas_helper` method. Contains useful test parameters.
struct CapabilityTestParams {
    pub bank: Bank<S>,
    pub transaction: Transaction<S>,
    pub gas_meter: TxGasMeter<<S as Spec>::Gas>,
    pub sender_address: <S as Spec>::Address,
    pub state_checkpoint: StateCheckpoint<S>,
}

/// Helper function that creates a simple bank setup (one account with `initial_balance`), generates a transaction
/// with the given gas parameters, reserves some gas, checks the resulting gas meter, and returns useful test parameters.
fn reserve_gas_helper(
    initial_balance: u64,
    max_priority_fee_bips: PriorityFeeBips,
    gas_limit: Option<GasUnit<2>>,
    gas_price: &<<S as Spec>::Gas as Gas>::Price,
    // The gas consumed by pre-execution checks
    gas_for_pre_execution_checks: &<S as Spec>::Gas,
) -> CapabilityTestParams {
    let (sender_address, bank, mut checkpoint) = simple_bank_setup(initial_balance);

    let transaction: Transaction<S> =
        generate_empty_tx(max_priority_fee_bips, initial_balance, gas_limit.clone());

    let mut pre_execution_checks_meter = UnlimitedGasMeter::new();
    pre_execution_checks_meter
        .charge_gas(gas_for_pre_execution_checks)
        .unwrap();

    // We try to reserve gas, this should succeed because we have enough balance.
    let gas_meter = bank
        .reserve_gas(
            &transaction.clone().into(),
            gas_price,
            &sender_address,
            &pre_execution_checks_meter,
            &mut checkpoint,
        )
        .expect("The reserve gas operation should not fail");

    let mut expected_meter =
        AuthenticatedTransactionData::from(transaction.clone()).gas_meter(gas_price);

    expected_meter
        .charge_gas(gas_for_pre_execution_checks)
        .expect("The reserve gas operation should take into account the pre-execution checks");

    assert_eq!(gas_meter, expected_meter);

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
/// We use half the price for pre-execution checks, the rest for the transaction.
#[test]
fn test_honest_reserve_gas_capability_without_priority_fee() {
    let initial_balance = 100;
    let mut params = reserve_gas_helper(
        initial_balance,
        PriorityFeeBips::ZERO,
        Some(GasUnit::from_slice(&[initial_balance / 2; 2])),
        &<<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]),
        &<S as Spec>::Gas::from_slice(&[initial_balance / 4; 2]),
    );

    // Let's consume all the gas, this should succeed because there is enough gas left in the meter.
    params
        .gas_meter
        .charge_gas(&GasUnit::from_slice(&[initial_balance / 4; 2]))
        .expect("The charge gas operation should not fail");

    let auth_tx: AuthenticatedTransactionData<S> = params.transaction.into();

    // We try to refund the gas, this should never fail. The gas is already consumed so the sender balance should be zero.
    let consumption = params.bank.consume_gas_and_allocate_rewards(
        &auth_tx,
        params.gas_meter,
        &params.bank.id().to_payable(),
        &params.bank.id().to_payable(),
        &mut params.state_checkpoint,
    );

    params.bank.refund_remaining_gas(
        &auth_tx,
        &params.sender_address,
        &consumption,
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
/// We use half the price for pre-execution checks, the rest for the transaction.
#[test]
fn test_honest_reserve_gas_capability_does_not_charge_priority_fee() {
    let initial_balance = 100;
    let mut params = reserve_gas_helper(
        initial_balance,
        PriorityFeeBips::from_percentage(10),
        Some(GasUnit::from_slice(&[initial_balance / 2; 2])),
        &<<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]),
        &<S as Spec>::Gas::from_slice(&[initial_balance / 4; 2]),
    );

    // Let's consume all the gas, this should succeed because there is enough gas left in the meter.
    params
        .gas_meter
        .charge_gas(&GasUnit::from_slice(&[initial_balance / 4; 2]))
        .expect("The charge gas operation should not fail");

    let auth_tx: AuthenticatedTransactionData<S> = params.transaction.into();

    // We try to refund the gas, this should never fail. The gas is already consumed so the sender balance should be zero.
    let consumption = params.bank.consume_gas_and_allocate_rewards(
        &auth_tx,
        params.gas_meter,
        &params.bank.id().to_payable(),
        &params.bank.id().to_payable(),
        &mut params.state_checkpoint,
    );

    params.bank.refund_remaining_gas(
        &auth_tx,
        &params.sender_address,
        &consumption,
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
    let max_priority_fee_bips = PriorityFeeBips::from_percentage(10);
    let gas_price = &<<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]);

    let mut params = reserve_gas_helper(
        initial_balance,
        PriorityFeeBips::from_percentage(10),
        Some(GasUnit::from_slice(&[initial_balance / 4; 2])),
        &<<S as Spec>::Gas as Gas>::Price::from_slice(&[1; 2]),
        &<<S as Spec>::Gas>::zero(),
    );

    let gas_to_charge = GasUnit::from_slice(&[initial_balance / 20; 2]);

    // We try to charge for some of the gas
    params
        .gas_meter
        .charge_gas(&gas_to_charge)
        .expect("The charge gas operation should not fail");

    let auth_tx: AuthenticatedTransactionData<S> = params.transaction.into();

    // We try to refund the gas, this should never fail. The gas is already consumed so the sender balance should be zero.
    let consumption = params.bank.consume_gas_and_allocate_rewards(
        &auth_tx,
        params.gas_meter,
        &params.bank.id().to_payable(),
        &params.bank.id().to_payable(),
        &mut params.state_checkpoint,
    );

    params.bank.refund_remaining_gas(
        &auth_tx,
        &params.sender_address,
        &consumption,
        &mut params.state_checkpoint,
    );

    // The sender should have been refunded:
    // initial_balance - gas_to_charge_value * (1 + max_priority_fee_percentage)
    let gas_to_charge_value = gas_to_charge.value(gas_price);

    let refund_amount = initial_balance
        - gas_to_charge_value
        - max_priority_fee_bips
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

/// Tests that the `reserve_gas` method fails if the sender does not have a bank account for the gas token
#[test]
fn test_reserve_gas_no_account() {
    let (_, bank, mut checkpoint) = simple_bank_setup(0);

    // This transaction has a maximum fee of twice the initial balance.
    let transaction: Transaction<S> = generate_empty_tx(PriorityFeeBips::ZERO, 0, None);

    // We try to reserve gas, this should fail because we have not enough balance.
    let reserve_gas_result = bank
        .reserve_gas(
            &transaction.into(),
            &<<S as Spec>::Gas as Gas>::Price::ZEROED,
            &Address::new([0u8; 32]),
            &UnlimitedGasMeter::new(),
            &mut checkpoint,
        )
        .expect_err("The reserve gas operation should fail");

    assert_eq!(reserve_gas_result, ReserveGasError::AccountDoesNotExist);
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
        .reserve_gas(
            &transaction.into(),
            &gas_price,
            &sender_address,
            &UnlimitedGasMeter::new(),
            &mut checkpoint,
        )
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
        .reserve_gas(
            &transaction.into(),
            &gas_price,
            &sender_address,
            &UnlimitedGasMeter::new(),
            &mut checkpoint,
        )
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
        &<S as Spec>::Gas::zero(),
    );

    let auth_tx: AuthenticatedTransactionData<S> = params.transaction.into();

    // We try to refund the gas, this should never fail. The gas is already consumed so the sender balance should be zero.
    let consumption = params.bank.consume_gas_and_allocate_rewards(
        &auth_tx,
        params.gas_meter,
        &params.bank.id().to_payable(),
        &params.bank.id().to_payable(),
        &mut params.state_checkpoint,
    );

    params.bank.refund_remaining_gas(
        &auth_tx,
        &params.sender_address,
        &consumption,
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
        &<S as Spec>::Gas::zero(),
    );

    let auth_tx: AuthenticatedTransactionData<S> = params.transaction.into();

    // We try to refund the gas, this should never fail. The gas is already consumed so the sender balance should be zero.
    let consumption = params.bank.consume_gas_and_allocate_rewards(
        &auth_tx,
        params.gas_meter,
        &params.bank.id().to_payable(),
        &params.bank.id().to_payable(),
        &mut params.state_checkpoint,
    );

    params.bank.refund_remaining_gas(
        &auth_tx,
        &params.sender_address,
        &consumption,
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
