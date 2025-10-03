use zk_poc::{CallMessage, Response, ZkPoc, ZkPocConfig};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_optimistic_runtime, AsUser, TransactionTestCase};
use sov_risc0_adapter::host::Risc0Host;
use sov_rollup_interface::zk::{CodeCommitment, ZkvmHost};
use std::time::Instant;
use serial_test::serial;

generate_optimistic_runtime!(ZkPocRuntime <= zk_poc: ZkPoc<S>);

type S = sov_test_utils::TestSpec;
type RT = ZkPocRuntime<S>;

#[test]
#[serial]
fn test_set_even_value_with_valid_proof() {
    println!("test_set_even_value_with_valid_proof");
    if zk_poc_risc0_methods::EVEN_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built (EVEN_ELF empty). Install toolchain or unset SKIP_GUEST_BUILD.");
        return;
    }

    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let user = genesis_config
        .additional_accounts()
        .first()
        .unwrap()
        .clone();


    println!("EVEN_ELF len = {}", zk_poc_risc0_methods::EVEN_ELF.len());

    // Generate a real RISC0 receipt for value = 100 using the 'even' guest.
    let value: u64 = 100;
    let mut host = Risc0Host::new(zk_poc_risc0_methods::EVEN_ELF);
    host.add_hint(value);
    let start = Instant::now();
    let method_id_vec = host.code_commitment().encode();
    let elapsed = start.elapsed();
    println!("code_commitment().encode() took: {:?}", elapsed);

    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), ZkPocConfig { method_id });
    println!("genesis processed");

    let mut runner = TestRunner::<_, _>::new_with_genesis(
        genesis.into_genesis_params(),
        ZkPocRuntime::default(),
    );

    // Produce the proof (receipt) bytes
    let proof_bytes = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");
    println!("proof generated");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ZkPoc<S>>(CallMessage::SetValue { value, proof: proof_bytes.try_into().unwrap() }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                ZkPocRuntimeEvent::ZkPoc(zk_poc::Event::Set { value: 100 })
            );
        }),
    });

    println!("query_visible_state");
    runner.query_visible_state(|state| {
        assert_eq!(
            ZkPoc::<S>::default().query_value(state),
            Response { value: Some(100) }
        );
    });
    println!("query_visible_state done");
}

#[test]
#[serial]
fn test_reject_odd_value() {
    if zk_poc_risc0_methods::EVEN_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built (EVEN_ELF empty). Install toolchain or unset SKIP_GUEST_BUILD.");
        return;
    }
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_config
        .additional_accounts()
        .first()
        .unwrap()
        .clone();

    // Prepare method id using a valid even value, but we will call with a mismatched odd value
    let even_value: u64 = 4;
    let mut host = Risc0Host::new(zk_poc_risc0_methods::EVEN_ELF);
    host.add_hint(even_value);
    let start = Instant::now();
    let method_id_vec = host.code_commitment().encode();
    let elapsed = start.elapsed();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), ZkPocConfig { method_id });
    let mut runner = TestRunner::<_, _>::new_with_genesis(
        genesis.into_genesis_params(),
        ZkPocRuntime::default(),
    );

    // Generate a proof for an even value but submit an odd value in the call; should fail
    let proof_bytes = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");
    let value: u64 = 5;

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ZkPoc<S>>(CallMessage::SetValue { value, proof: proof_bytes.try_into().unwrap() }),
        assert: Box::new(|result, _state| {
            assert!(!result.tx_receipt.is_successful());
        }),
    });
}
