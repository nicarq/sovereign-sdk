use crate::helpers::{create_contract_tx, create_set_arg_tx, setup};
use crate::runtime::{RT, S};
use alloy_primitives::U256;
use sov_evm::{EthereumAuthenticator, Evm};
use sov_test_utils::{BatchTestCase, SimpleStorageContract, TransactionType};

#[test]
fn test_account_abstraction() {
    let (mut runner, _, account, _) = setup();
    let contract = SimpleStorageContract::default();
    let contract_addr = account.address().create(0);
    let create_contract_tx = create_contract_tx(0, &contract, &account);

    let set_value_tx = create_set_arg_tx(5, 1, &contract, contract_addr, &account);

    runner.execute_batch(BatchTestCase {
        input: vec![
            TransactionType::<RT, S>::PreAuthenticated(RT::encode_with_ethereum_auth(
                create_contract_tx,
            )),
            TransactionType::<RT, S>::PreAuthenticated(RT::encode_with_ethereum_auth(set_value_tx)),
        ]
        .into(),
        assert: Box::new(move |_result, state| {
            let evm = Evm::<S>::default();
            let receipts = evm.receipts(state);
            assert_eq!(receipts.len(), 2);
            for receipt in receipts {
                assert!(
                    receipt.receipt.success,
                    "Eth tx didn't execute successfully, receipt: {receipt:?}"
                );
            }
            let storage_value = evm
                .get_storage(&contract_addr, &U256::ZERO, state)
                .unwrap()
                .unwrap();
            assert_eq!(U256::from(5), storage_value);
        }),
    });

    for n in 2..10 {
        let address = account.address();
        let set_value_tx =
            create_set_arg_tx((n + 90) as u32, n, &contract, contract_addr, &account);

        runner.execute_batch(BatchTestCase {
            input: vec![TransactionType::<RT, S>::PreAuthenticated(
                RT::encode_with_ethereum_auth(set_value_tx),
            )]
            .into(),
            assert: Box::new(move |_result, state| {
                let evm = Evm::<S>::default();
                let nonce_from_module = evm
                    .get_transaction_count(address, None, state)
                    .unwrap()
                    .to::<u64>();
                assert_eq!(n + 1, nonce_from_module);

                let storage_value = evm
                    .get_storage(&contract_addr, &U256::ZERO, state)
                    .unwrap()
                    .unwrap();
                assert_eq!(U256::from(n + 90), storage_value);
            }),
        });
    }
}
