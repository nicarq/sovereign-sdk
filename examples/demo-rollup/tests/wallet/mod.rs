use std::str::FromStr;

use demo_stf::runtime::RuntimeCall;
use sov_bank::{CallMessage, Coins, TokenId};
use sov_mock_da::MockDaSpec;
use sov_mock_zkvm::MockZkVerifier;
use sov_modules_api::default_spec::DefaultSpec;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::sov_wallet_format::compiled_schema::CompiledSchema;
use sov_modules_api::Spec;

type Da = MockDaSpec;
type S = DefaultSpec<MockZkVerifier, MockZkVerifier, Native>;

#[test]
fn test_display_tx() {
    let msg: RuntimeCall<S, Da> = RuntimeCall::Bank(CallMessage::Transfer {
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
    let schema = CompiledSchema::of::<RuntimeCall<S, Da>>();
    assert_eq!(
        schema.display(&data).unwrap(),
        r#"Bank.Transfer { to: sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9stup8tx, coins: { amount: 10000, token_id: token_1zut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzut3w9chzutsuzalks}}"#
    );
}
