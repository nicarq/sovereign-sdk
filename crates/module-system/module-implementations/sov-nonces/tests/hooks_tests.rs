use sov_modules_api::capabilities::config_chain_id;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::UnsignedTransaction;
use sov_modules_api::{CredentialId, EncodeCall, TxEffect};
use sov_nonces::Nonces;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{TestRunner, ValueSetter, ValueSetterConfig};
use sov_test_utils::{
    generate_optimistic_runtime, TestUser, TransactionTestCase, TransactionType, TxProcessingError,
    TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE,
};

type S = sov_test_utils::TestSpec;

generate_optimistic_runtime!(TestNonceRuntime <= value_setter: ValueSetter<S>);

fn generate_default_tx(nonce: u64, admin: &TestUser<S>) -> TransactionType<ValueSetter<S>, S> {
    let runtime_msg = <TestNonceRuntime<S> as EncodeCall<ValueSetter<S>>>::encode_call(
        sov_value_setter::CallMessage::SetValue(10),
    );

    let transaction = UnsignedTransaction::new(
        runtime_msg,
        config_chain_id(),
        TEST_DEFAULT_MAX_PRIORITY_FEE,
        TEST_DEFAULT_MAX_FEE,
        nonce,
        None,
    );

    TransactionType::pre_signed(transaction, admin.private_key())
}

fn setup() -> (TestUser<S>, TestRunner<TestNonceRuntime<S>, S>) {
    // Generate a genesis config, then overwrite the attester key/address with ones that
    // we know. We leave the other values untouched.
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let admin = genesis_config.additional_accounts.first().unwrap().clone();

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(
        genesis_config.into(),
        ValueSetterConfig {
            admin: admin.address(),
        },
    );

    let runner =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), TestNonceRuntime::default());

    (admin, runner)
}

#[test]
fn send_tx_works() {
    let (admin, mut runner) = setup();

    let admin_credential_id: CredentialId = admin.credential_id();

    runner.query_visible_state(|state| {
        assert_eq!(
            Nonces::<S>::default()
                .nonce(&admin_credential_id, state)
                .unwrap_infallible(),
            None,
            "The nonce should not be set"
        );
    });

    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(0, &admin),
        assert: Box::new(move |ctx, state| {
            assert!(ctx.tx_receipt.is_successful());

            assert_eq!(
                Nonces::<S>::default()
                    .nonce(&admin_credential_id, state)
                    .unwrap_infallible(),
                Some(1),
                "The nonce should be 1"
            );
        }),
    });

    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(1, &admin),
        assert: Box::new(move |ctx, state| {
            assert!(ctx.tx_receipt.is_successful());
            assert_eq!(
                Nonces::<S>::default()
                    .nonce(&admin_credential_id, state)
                    .unwrap_infallible(),
                Some(2),
                "The nonce should be 2"
            );
        }),
    });
}

#[test]
fn send_tx_bad_nonce() {
    let (admin, mut runner) = setup();

    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(5, &admin),
        assert: Box::new(move |ctx, _state| {
            if let TxEffect::Skipped(skipped) = &ctx.tx_receipt {
                assert!(matches!(
                    skipped.error,
                    TxProcessingError::IncorrectNonce(_)
                ));
            } else {
                panic!(
                    "Expected Skipped error, but got a different TxEffect: {:?}",
                    ctx.tx_receipt
                );
            }
        }),
    });
}
