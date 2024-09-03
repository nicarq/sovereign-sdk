use sov_bank::{Bank, BankGasConfig, CallMessage, GAS_TOKEN_ID};
use sov_chain_state::ChainState;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Error, Gas, GasArray, Spec, TxEffect};
use sov_test_utils::{
    AsUser, AtomicNumber, MockDaSpec, TransactionTestAssert, TransactionTestCase,
    TEST_DEFAULT_USER_BALANCE,
};

use crate::helpers::{setup_with_custom_runtime, TestData, RT, S};

/// Additional context for the post-call assertions of the gas tests.
/// This is used to check the outcome of the create token call.
struct PostCreateTokenContext {
    user_initial_balance: u64,
    user_address: <S as Spec>::Address,
}

/// A helper function that creates a token with a custom gas config and checks the balance of the user
/// after the call.
/// The gas config can be set to `None` to use the default gas config.
fn gas_test_setup(
    gas_to_charge_for_create_token: Option<<S as Spec>::Gas>,
    create_token_assert: impl FnOnce(PostCreateTokenContext) -> TransactionTestAssert<S, RT> + 'static,
) {
    let (
        TestData {
            user_high_token_balance: user,
            ..
        },
        mut runner,
    ) = setup_with_custom_runtime(|runtime| {
        if let Some(gas_to_charge_for_create_token) = gas_to_charge_for_create_token {
            let config = BankGasConfig {
                create_token: gas_to_charge_for_create_token,
                transfer: Gas::zero(),
                burn: Gas::zero(),
                mint: Gas::zero(),
                freeze: Gas::zero(),
            };

            runtime.bank.override_gas_config(config);
        }
    });

    let user_balance = user.available_gas_balance;

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<Bank<S>>(CallMessage::CreateToken {
            salt: 0,
            token_name: "sov-test-token".to_string(),
            initial_balance: 1000,
            mint_to_address: user.address(),
            authorized_minters: vec![],
        }),
        assert: Box::new(move |result, state| {
            create_token_assert(PostCreateTokenContext {
                user_initial_balance: user_balance,
                user_address: user.address(),
            })(result, state);
        }),
    });
}

/// Test that the gas price constants are charged correctly for the bank runtime.
/// To do that we override the gas config, set the costs to zero, execute a call and store the gas consumed.
/// Then we try with a different runtime config and check that the gas consumed only increases by the amount specified in the second config.
#[test]
fn gas_price_constants_are_charged_correctly() {
    let gas_consumed_without_price_ref = AtomicNumber::new(0);
    let gas_consumed_without_price_ref_1 = gas_consumed_without_price_ref.clone();

    gas_test_setup(
        Some(<S as Spec>::Gas::from_slice(&[0; 2])),
        move |PostCreateTokenContext {
                  user_address,
                  user_initial_balance,
              }| {
            Box::new(move |result, state| {
                assert_eq!(result.tx_receipt, TxEffect::Successful(()));

                let user_final_balance = Bank::<S>::default()
                    .get_balance_of(&user_address, GAS_TOKEN_ID, state)
                    .unwrap_infallible()
                    .unwrap();

                assert_eq!(
                    user_final_balance,
                    user_initial_balance - result.gas_value_used,
                    "the balance should decrease only by the gas used"
                );

                gas_consumed_without_price_ref.add(result.gas_value_used);
            })
        },
    );

    let gas_to_charge_for_create_token = <S as Spec>::Gas::from_slice(&[100; 2]);
    let bank_initial_gas_price = ChainState::<S, MockDaSpec>::initial_base_fee_per_gas();

    gas_test_setup(
        Some(gas_to_charge_for_create_token.clone()),
        move |PostCreateTokenContext {
                  user_initial_balance,
                  user_address,
              }| {
            Box::new(move |result, state| {
                assert_eq!(result.tx_receipt, TxEffect::Successful(()));

                let user_final_balance = Bank::<S>::default()
                    .get_balance_of(&user_address, GAS_TOKEN_ID, state)
                    .unwrap_infallible()
                    .unwrap();

                assert_eq!(
                    user_final_balance,
                    user_initial_balance - result.gas_value_used,
                    "the balance should decrease only by the gas used"
                );

                assert_eq!(
                gas_consumed_without_price_ref_1.get()
                    + gas_to_charge_for_create_token.value(&bank_initial_gas_price),
                result.gas_value_used,
                "The gas used should be the sum of the gas cost of the call and the inner gas cost"
            );
            })
        },
    );
}

#[test]
fn config_constants_are_charged_correctly() {
    let gas_consumed_without_price_ref = AtomicNumber::new(0);
    let gas_consumed_without_price_ref_1 = gas_consumed_without_price_ref.clone();

    // compute the expected gas cost, based on the json constants
    let create_token_config_cost = Bank::<S>::default().gas_config().create_token.clone();
    let bank_initial_gas_price = ChainState::<S, MockDaSpec>::initial_base_fee_per_gas();

    gas_test_setup(
        Some(<S as Spec>::Gas::from_slice(&[0; 2])),
        move |PostCreateTokenContext {
                  user_initial_balance,
                  user_address,
              }| {
            Box::new(move |result, state| {
                assert_eq!(result.tx_receipt, TxEffect::Successful(()));

                let user_final_balance = Bank::<S>::default()
                    .get_balance_of(&user_address, GAS_TOKEN_ID, state)
                    .unwrap_infallible()
                    .unwrap();

                assert_eq!(
                    user_final_balance,
                    user_initial_balance - result.gas_value_used,
                    "the balance should be unchanged with zeroed price"
                );

                gas_consumed_without_price_ref.add(result.gas_value_used);
            })
        },
    );

    gas_test_setup(
        None,
        move |PostCreateTokenContext {
                  user_initial_balance,
                  user_address,
              }| {
            Box::new(move |result, state| {
                assert_eq!(result.tx_receipt, TxEffect::Successful(()));

                let user_final_balance = Bank::<S>::default()
                    .get_balance_of(&user_address, GAS_TOKEN_ID, state)
                    .unwrap_infallible()
                    .unwrap();

                assert_eq!(
                    user_final_balance,
                    user_initial_balance - result.gas_value_used,
                    "the balance should be unchanged with zeroed price"
                );

                assert_eq!(
                gas_consumed_without_price_ref_1.get()
                    + create_token_config_cost.value(&bank_initial_gas_price),
                result.gas_value_used,
                "The gas used should be the sum of the gas cost of the call and the inner gas cost"
            );
            })
        },
    );
}

#[test]
fn not_enough_gas_wont_panic() {
    gas_test_setup(
        Some(<S as Spec>::Gas::from_slice(&[
            TEST_DEFAULT_USER_BALANCE / 2,
            TEST_DEFAULT_USER_BALANCE / 2,
        ])),
        |_| {
            Box::new(move |result, _state| {
                assert!(
                    matches!(result.tx_receipt, TxEffect::Reverted(..)),
                    "The transaction outcome is incorrect"
                );

                if let TxEffect::Reverted(Error::ModuleError(err)) = result.tx_receipt {
                    let mut chain = err.chain();
                    assert_eq!(chain.len(), 1, "The error chain is incorrect");

                    assert!(
                        chain.next().unwrap().to_string().contains(
                            "The gas to charge is greater than the funds available in the meter."
                        ),
                        "The error message is incorrect"
                    );
                } else {
                    panic!("The transaction outcome is incorrect")
                }
            })
        },
    );
}

#[test]
fn very_high_gas_to_charge_wont_panic_or_overflow() {
    gas_test_setup(
        Some(<S as Spec>::Gas::from_slice(&[u64::MAX - 1, u64::MAX - 1])),
        |_| {
            Box::new(move |result, _state| {
                assert!(
                    matches!(result.tx_receipt, TxEffect::Reverted(..)),
                    "The transaction outcome is incorrect"
                );

                if let TxEffect::Reverted(Error::ModuleError(err)) = result.tx_receipt {
                    let mut chain = err.chain();
                    assert_eq!(chain.len(), 1, "The error chain is incorrect");

                    assert!(
                        chain.next().unwrap().to_string().contains(
                            "The gas to charge is greater than the funds available in the meter."
                        ),
                        "The error message is incorrect"
                    );
                } else {
                    panic!("The transaction outcome is incorrect")
                }
            })
        },
    );
}
