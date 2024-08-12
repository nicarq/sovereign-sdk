use sov_modules_api::sov_wallet_format::compiled_schema::CompiledSchema;
use sov_prover_incentives::CallMessage;

#[derive(Debug, PartialEq, borsh::BorshSerialize, sov_modules_api::macros::UniversalWallet)]
enum RuntimeCall {
    ProverIncentives(CallMessage),
}

#[test]
fn test_display_accounts_call() {
    let msg = RuntimeCall::ProverIncentives(CallMessage::Register(100));

    let schema = CompiledSchema::of::<RuntimeCall>();
    assert_eq!(
        schema.display(&borsh::to_vec(&msg).unwrap()).unwrap(),
        r#"ProverIncentives.Register(100)"#
    );
}
