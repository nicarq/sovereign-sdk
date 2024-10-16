use sov_paymaster::{PayeePolicy, PaymasterConfig, PaymasterPolicy, PaymasterSetup};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{
    TestRunner, ValueSetter, ValueSetterCallMessage, ValueSetterConfig, ValueSetterEvent,
};
use sov_test_utils::{AsUser, TestUser, TransactionTestCase};

use crate::runtime::{GenesisConfig, PaymasterRuntime, PaymasterRuntimeEvent};

type S = sov_test_utils::TestSpec;

// Test that a transaction for a user succeeds even when the user has no balance to pay for gas
// if the paymaster is willing to cover that user.
#[test]
fn test_happy_path() {
    // Generate a genesis config
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(2);

    let sequencer = genesis_config.initial_sequencer.da_address;
    let payer = genesis_config.additional_accounts.first().unwrap().clone();
    let user = TestUser::generate(0);

    let genesis = GenesisConfig::from_minimal_config(
        genesis_config.into(),
        PaymasterConfig {
            payers: vec![PaymasterSetup {
                payer_address: payer.address(),
                policy: PaymasterPolicy {
                    default_payee_policy: PayeePolicy::Allow {
                        max_fee: None,
                        gas_limit: None,
                        max_gas_price: None,
                    },
                    payees: vec![],
                    authorized_sequencers: sov_paymaster::AuthorizedSequencers::All,
                    authorized_updaters: vec![payer.address()],
                },
                sequencers_to_register: vec![sequencer],
            }],
        },
        ValueSetterConfig {
            admin: user.address(),
        },
    );

    let mut runner =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), PaymasterRuntime::default());

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<ValueSetter<S>>(ValueSetterCallMessage::SetValue(99)),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());

            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                PaymasterRuntimeEvent::ValueSetter(ValueSetterEvent::NewValue(99))
            );
        }),
    });
}

// -[] Add missing tests: <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1618>
//   -[] Register paymaster using callmessage
//   -[] Set payer for sequencer using callmessage
//   -[] Register payee for user
//   -[] Update payee policy for a user
//   -[] Remove payee policy for a user
//   -[] Remove payer for sequencer
//   -[] Remove payer
//   -[] Update authorized sequencers
//   -[] Update authorized updaters
//   -[] Update default payee policy
//   -[x] Test happy path - paymaster pays with default policy ✅
//   -[] Test happy path - paymaster pays with special policy
//   -[] Test unhappy path - paymaster does not have enough balance
//   -[] Test unhappy path - paymaster is not registered
//   -[] Test unhappy path - paymaster is not authorized to pay for sequencer
//   -[] Test unhappy path - paymaster is not authorized to pay for user
//     -[] Gas price too high
//     -[] Gas limit too high
//     -[] Max fee too high
//     -[] Denied
//   -[] Test unhappy path - user pays when paymaster does not. In this case, paymaster balance must be unchanged
// -[] Add back this test once the wallet schema supports DA address types.
// #[test]
// fn test_display_paymaster_call() {
//     let msg = PaymasterRuntimeCall::Paymaster(CallMessage::SetPayerForSequencer {
//         payer: "sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94",
//     });

//     let schema = Schema::of::<PaymasterRuntimeCall<S>>();
//     assert_eq!(
//         schema.display(&borsh::to_vec(&msg).unwrap()).unwrap(),
//         r#"ValueSetter.SetValue(5)"#
//     );
// }
