use crate::helpers::*;
use crate::runtime::RT;
use crate::runtime::S;
use alloy_primitives::FixedBytes;
use alloy_primitives::U256;
use alloy_rpc_types::BlockTransactions;
use alloy_rpc_types_trace::geth::GethDebugTracingOptions;
use alloy_rpc_types_trace::geth::GethTrace;
use reth_primitives::Log;
use revm::Database;
use sov_evm::Evm;
use sov_modules_api::GasArray;
use sov_test_utils::TransactionType;
use sov_test_utils::{BatchTestCase, SimpleStorageContract};
use sov_test_utils::{TransactionTestCase, TEST_DEFAULT_USER_BALANCE};

#[test]
fn test_tracing() {
    let (mut runner, account, _) = setup();
    let contract = SimpleStorageContract::default();
    let contract_addr = account.address().create(0);

    let address = account.address();
    let mut txs = vec![create_deploy_tx(0, &contract, &account).tx];

    let mut nonce = 1;
    let mut tx_hash = None;
    for x in 1..10 {
        for _ in 1..10 {
            let tx = create_set_arg_tx(
                (nonce + 100) as u32, // Add 100 so that the value set in the smart contract differs from the nonce.
                nonce,
                &contract,
                contract_addr,
                &account,
            );
            if nonce == 48 {
                println!("xx {:?}", x);
                tx_hash = Some(tx.hash);
            }
            txs.push(tx.tx);

            nonce += 1;
        }

        runner.execute_batch(BatchTestCase {
            input: txs.into(),
            assert: Box::new(move |ctx, state| {
                for r in ctx.batch_receipt.unwrap().tx_receipts {
                    assert!(r.receipt.is_successful())
                }
            }),
        });

        txs = vec![];
    }

    let evm = Evm::<S>::default();
    let slot_height_1 = runner.query_state(|state| {
        //

        let opts = Some(GethDebugTracingOptions::default());
        evm.debug_trace_transaction(tx_hash.unwrap(), opts, state);
    });
}
