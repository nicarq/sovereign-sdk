use borsh::BorshDeserialize;
use sov_attester_incentives::CallMessage;
use sov_modules_api::sov_wallet_format::compiled_schema::CompiledSchema;

#[derive(
    Debug,
    Clone,
    PartialEq,
    borsh::BorshSerialize,
    BorshDeserialize,
    sov_modules_api::macros::UniversalWallet,
)]
pub enum RuntimeCall {
    AttesterIncentives(CallMessage),
}

#[test]
fn test_display_bond_attester() {
    let msg = RuntimeCall::AttesterIncentives(CallMessage::RegisterAttester(100));
    let schema = CompiledSchema::of::<RuntimeCall>();
    assert_eq!(
        schema.display(&borsh::to_vec(&msg).unwrap()).unwrap(),
        r#"AttesterIncentives.RegisterAttester(100)"#
    );
}
