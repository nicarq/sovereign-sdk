use sov_modules_api::prelude::tokio;
use sov_modules_api::{Gas, GasSpec, Spec};

use crate::{TestData, S};

/// Tests that getting the latest base fee per gas returns the initial base fee per gas after genesis.
#[tokio::test(flavor = "multi_thread")]
async fn test_get_base_fee_per_gas_latest() {
    let data = TestData::setup().await;

    let response = data.client().get_latest_base_fee_per_gas().await.unwrap();
    assert_eq!(
        <<S as Spec>::Gas as Gas>::Price::try_from(
            response.data.clone().unwrap().base_fee_per_gas.0
        )
        .unwrap(),
        S::initial_base_fee_per_gas()
    );
}

// Tests that getting the latest base fee per gas gets updated after a slot is processed.
// TODO(@theochap): uncomment this test once <`https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1552`> is fixed
// #[tokio::test(flavor = "multi_thread")]
// async fn test_get_base_fee_per_gas_latest_with_updates() {
//     let mut data = TestData::setup().await;

//     let initial_response = data
//         .client()
//         .get_latest_base_fee_per_gas()
//         .await
//         .unwrap()
//         .data
//         .clone()
//         .unwrap();

//     let runner = &mut data.runner;
//     let user = &data.user;

//     for _ in 0..5 {
//         runner.execute_transaction(TransactionTestCase {
//             input: user.create_plain_message::<Bank<S>>(sov_bank::CallMessage::Burn {
//                 coins: sov_bank::Coins {
//                     amount: 1000,
//                     token_id: GAS_TOKEN_ID,
//                 },
//             }),
//             assert: Box::new(move |result, _state| {
//                 assert!(
//                     result.tx_receipt.is_successful(),
//                     "The transaction should have succeeded"
//                 );
//             }),
//         });
//     }

//     let current_gas_price = runner
//         .receipts()
//         .last()
//         .unwrap()
//         .last_batch_receipt()
//         .gas_price
//         .clone();

//     let initial_gas_price = S::initial_base_fee_per_gas();

//     assert!(
//         current_gas_price < initial_gas_price,
//         "The gas price in the runner should have decreased! Current gas price {current_gas_price}, initial gas price {initial_gas_price}"
//     );

//     data.send_storage();

//     let response = data
//         .client()
//         .get_latest_base_fee_per_gas()
//         .await
//         .unwrap()
//         .data
//         .clone()
//         .unwrap();

//     let api_initial_gas_price =
//         <<S as Spec>::Gas as Gas>::Price::try_from(initial_response.base_fee_per_gas.0).unwrap();
//     let api_current_gas_price =
//         <<S as Spec>::Gas as Gas>::Price::try_from(response.base_fee_per_gas.0).unwrap();

//     // The gas price should match the initial gas price
//     assert_eq!(api_initial_gas_price, initial_gas_price);

//     // The gas price should decrease because the slot doesn't have enough gas
//     assert!(
//         api_current_gas_price < api_initial_gas_price,
//         "The gas price should have decreased, but it didn't: current gas price {api_current_gas_price}, initial gas price {api_initial_gas_price}"
//     );

//     assert_eq!(
//         api_current_gas_price, current_gas_price,
//         "The api gas price should be the same as the current gas price"
//     );
// }
