use sov_modules_api::sov_wallet_format::compiled_schema::CompiledSchema;
use sov_sequencer_registry::CallMessage;

#[derive(Debug, PartialEq, borsh::BorshSerialize, sov_modules_api::macros::UniversalWallet)]
enum RuntimeCall {
    SequencerRegistry(CallMessage),
}

#[test]
fn test_display_sequencer_registry_call() {
    let msg = RuntimeCall::SequencerRegistry(CallMessage::Deposit {
        da_address: vec![1, 2, 3, 4],
        amount: 100,
    });

    let schema = CompiledSchema::of::<RuntimeCall>();
    assert_eq!(
        schema.display(&borsh::to_vec(&msg).unwrap()).unwrap(),
        "SequencerRegistry.Deposit { da_address: 0x01020304, amount: 100}"
    );
}
