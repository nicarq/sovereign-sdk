use std::str::FromStr;

use demo_stf::runtime::RuntimeCall;
use sov_bank::{CallMessage, Coins, TokenId};
use sov_mock_da::MockDaSpec;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::default_spec::DefaultSpec;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::sov_universal_wallet::schema::{RollupRoots, Schema};
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::Spec;

type S = DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, Native>;

#[test]
fn test_display_tx() {
    let msg: RuntimeCall<S> = RuntimeCall::Bank(CallMessage::Transfer {
        to: <S as Spec>::Address::from_str(
            "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9stup8tx",
        )
        .unwrap(),
        coins: Coins {
            amount: 10_000,
            token_id: TokenId::from_str(
                "token_1zut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzutsuzalks",
            )
            .unwrap(),
        },
    });
    let data = borsh::to_vec(&msg).unwrap();
    let schema = Schema::of_rollup_types_with_metadata::<
        u64,
        Transaction<S>,
        UnsignedTransaction<S>,
        RuntimeCall<S>,
    >(&4321)
    .unwrap();
    assert_eq!(
        schema
            .display(
                schema
                    .rollup_expected_index(RollupRoots::RuntimeCall)
                    .unwrap(),
                &data
            )
            .unwrap(),
        r#"Bank.Transfer to address sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9stup8tx 10000 coins of token ID token_1zut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzutsuzalks."#
    );
}
