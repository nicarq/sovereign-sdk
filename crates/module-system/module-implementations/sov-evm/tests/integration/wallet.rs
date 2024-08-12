use borsh::BorshDeserialize;
use sov_evm::{CallMessage, RlpEvmTransaction};
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
    Evm(CallMessage),
}

#[test]
fn test_display_evm() {
    let msg: RuntimeCall = RuntimeCall::Evm(CallMessage {
        rlp: RlpEvmTransaction { rlp: vec![1, 2, 3] },
    });
    let schema = CompiledSchema::of::<RuntimeCall>();
    assert_eq!(
        schema.display(&borsh::to_vec(&msg).unwrap()).unwrap(),
        r#"Evm { rlp: { rlp: 0x010203}}"#
    );
}
