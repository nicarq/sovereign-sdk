use sov_mock_da::MockDaSpec;
use sov_modules_api::capabilities::FatalError;
use sov_modules_api::macros::config_value;
use sov_modules_api::transaction::UnsignedTransaction;
use sov_modules_api::EncodeCall;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    generate_optimistic_runtime, BatchTestCase, TestUser, TransactionTestCase, TransactionType,
    TEST_DEFAULT_USER_BALANCE,
};
use sov_value_setter::{CallMessage, ValueSetter};

type S = sov_test_utils::TestSpec;

#[test]
fn test_enforces_chain_id() {
    generate_optimistic_runtime!(IntegTestRuntime <= value_setter: ValueSetter<S>);

    let mut genesis_config = HighLevelOptimisticGenesisConfig::generate();
    genesis_config
        .additional_accounts
        .push(TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE));

    let admin_account = genesis_config.additional_accounts[0].clone();

    let genesis = GenesisConfig::from_minimal_config(
        genesis_config.clone().into(),
        sov_value_setter::ValueSetterConfig {
            admin: admin_account.address(),
        },
    );

    let mut runner: TestRunner<IntegTestRuntime<S, MockDaSpec>, S> =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), Default::default());
    let encoded_message =
        <IntegTestRuntime<S, MockDaSpec> as EncodeCall<ValueSetter<S>>>::encode_call(
            CallMessage::SetValue(8),
        );

    let real_chain_id = config_value!("CHAIN_ID");

    let utx = UnsignedTransaction::new(
        encoded_message.clone(),
        real_chain_id,
        100.into(),
        100_000_000,
        0,
        None,
    );
    let tx = TransactionType::<ValueSetter<S>, S>::pre_signed(utx, admin_account.private_key());

    runner.execute_transaction(TransactionTestCase {
        input: tx,
        assert: Box::new(move |result, _state| assert!(result.tx_receipt.is_successful())),
    });

    let fake_chain_id = real_chain_id + 1;
    let invalid_utx = UnsignedTransaction::new(
        encoded_message,
        fake_chain_id,
        100.into(),
        100_000_000,
        0,
        None,
    );
    let tx =
        TransactionType::<ValueSetter<S>, S>::pre_signed(invalid_utx, admin_account.private_key());

    runner.execute_batch(BatchTestCase {
        input: vec![tx].into(),
        override_sequencer: None,
        assert: Box::new(move |result, _state| {
            match &result.batch_receipt.unwrap().inner.outcome {
                sov_modules_api::BatchSequencerOutcome::Slashed(reason) => {
                    assert_eq!(
                        reason,
                        &FatalError::InvalidChainId {
                            expected: 4321,
                            got: 4322
                        },
                        "Expected invalid chain id error but got {:?}",
                        reason
                    );
                }
                unexpected => panic!("Expected slashed outcome but got {:?}", unexpected),
            };
        }),
    });
}
