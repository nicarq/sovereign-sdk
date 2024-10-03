use borsh::BorshDeserialize;
use sov_attester_incentives::CallMessage;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::sov_universal_wallet::schema::Schema;

#[derive(Debug, Clone, PartialEq, borsh::BorshSerialize, BorshDeserialize, UniversalWallet)]
pub enum RuntimeCall {
    AttesterIncentives(CallMessage),
}

#[test]
fn test_display_bond_attester() {
    let msg = RuntimeCall::AttesterIncentives(CallMessage::RegisterAttester(100));
    let schema = Schema::of::<RuntimeCall>();
    assert_eq!(
        schema.display(&borsh::to_vec(&msg).unwrap()).unwrap(),
        r#"AttesterIncentives.RegisterAttester(100)"#
    );
}
