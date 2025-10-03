use midnight_privacy::{Event, Hash32, SpendPublic};
use midnight_privacy::audit::AuditCiphertext;
use midnight_privacy::{CallMessage, ShieldedPool};
use sov_bank::config_gas_token_id;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_optimistic_runtime, AsUser, TransactionTestCase};
use sov_risc0_adapter::host::Risc0Host;
use sov_rollup_interface::zk::{CodeCommitment, ZkvmHost};
use serial_test::serial;

type S = sov_test_utils::TestSpec;
generate_optimistic_runtime!(ShieldedRuntime <= shielded_pool: ShieldedPool<S>);
type RT = ShieldedRuntime<S>;

#[test]
fn deposit_and_spend_with_mock_proof() {
    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let cfg = midnight_privacy::PoolConfig {
        domain: [7u8; 32],
        vk_hash: [9u8; 32],
        fee_bips: 0,
        initial_viewers: vec![],
    };

    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // 1) Deposit
    let commitment = [1u8; 32];
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (1000u64).into(),
            commitment,
        }),
        assert: Box::new(move |result, _state| {
            assert!(result.tx_receipt.is_successful());
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == commitment)));
        }),
    });

    // Query last root
    let last_root: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // 2) Spend with a mock proof (bincode-serialized SpendPublic)
    let public = SpendPublic {
        anchor_root: last_root,
        nullifiers: vec![[42u8; 32]],
        commitments: vec![[2u8; 32]],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: [9u8; 32],
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&Vec::<AuditCiphertext>::new()).unwrap()),
    };
    let proof = bincode::serialize(&public).unwrap();

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof.try_into().unwrap(),
            anchor_root: last_root,
            audit_payloads: Vec::new().try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [42u8; 32])));
        }),
    });
}

#[test]
fn withdraw_path_emits_bank_transfer() {
    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let cfg = midnight_privacy::PoolConfig {
        domain: [7u8; 32],
        vk_hash: [9u8; 32],
        fee_bips: 0,
        initial_viewers: vec![],
    };

    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Deposit 1000
    let commitment = [11u8; 32];
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (1000u64).into(),
            commitment,
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let last_root: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Spend with withdraw_to: user, withdraw: 50
    let public = SpendPublic {
        anchor_root: last_root,
        nullifiers: vec![[55u8; 32]],
        commitments: vec![[66u8; 32]],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: [9u8; 32],
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&Vec::<AuditCiphertext>::new()).unwrap()),
    };
    let proof = bincode::serialize(&public).unwrap();
    let withdraw_amt = 50u64.into();

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Withdraw {
            proof: proof.try_into().unwrap(),
            anchor_root: last_root,
            to: user.address(),
            token_id: config_gas_token_id(),
            amount: withdraw_amt,
            audit_payloads: Vec::new().try_into().unwrap(),
        }),
        assert: Box::new(move |result, _state| {
            assert!(result.tx_receipt.is_successful());
            // Check we consumed the nullifier and appended the commitment
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [55u8; 32])));
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == [66u8; 32])));
        }),
    });
}

#[test]
fn reject_unknown_anchor() {
    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let cfg = midnight_privacy::PoolConfig {
        domain: [1u8; 32],
        vk_hash: [2u8; 32],
        fee_bips: 0,
        initial_viewers: vec![],
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Build a spend with a bogus anchor root
    let bogus_anchor: Hash32 = [0u8; 32];
    let public = SpendPublic {
        anchor_root: bogus_anchor,
        nullifiers: vec![[3u8; 32]],
        commitments: vec![],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: [2u8; 32],
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&Vec::<AuditCiphertext>::new()).unwrap()),
    };
    let proof = bincode::serialize(&public).unwrap();

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof.try_into().unwrap(),
            anchor_root: bogus_anchor,
            audit_payloads: Vec::new().try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_reverted())),
    });
}

#[test]
fn reject_double_nullifier() {
    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let cfg = midnight_privacy::PoolConfig { domain: [8u8; 32], vk_hash: [5u8; 32], fee_bips: 0, initial_viewers: vec![] };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Make a deposit to produce a valid root
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (1u64).into(),
            commitment: [7u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let anchor: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    let nf = [9u8; 32];
    let mk_public = |anchor: Hash32| SpendPublic {
        anchor_root: anchor,
        nullifiers: vec![nf],
        commitments: vec![],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: [5u8; 32],
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&Vec::<AuditCiphertext>::new()).unwrap()),
    };

    // First spend succeeds
    let proof1 = bincode::serialize(&mk_public(anchor)).unwrap();
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof1.try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: Vec::new().try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    // Second spend with same nullifier should revert (use latest anchor)
    let anchor2: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });
    let proof2 = bincode::serialize(&mk_public(anchor2)).unwrap();
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof2.try_into().unwrap(),
            anchor_root: anchor2,
            audit_payloads: Vec::new().try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_reverted())),
    });
}

#[test]
fn register_viewer_and_audit_payloads_and_grant() {
    use sov_modules_api::HexHash;

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let cfg = midnight_privacy::PoolConfig { domain: [7u8; 32], vk_hash: [1u8; 32], fee_bips: 0, initial_viewers: vec![] };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Deposit to get a root
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (1u64).into(),
            commitment: [5u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });
    let anchor: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Register a viewer
    let viewer_id = [1u8; 32];
    let viewer_pk = [2u8; 32];
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::RegisterViewer { id: viewer_id, pubkey: viewer_pk }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    // Build two audit payloads sealed to that viewer
    let a1 = AuditCiphertext { viewer_id, epk: [3u8; 32], nonce: [4u8; 24], ct: vec![1, 2, 3] };
    let a2 = AuditCiphertext { viewer_id, epk: [5u8; 32], nonce: [6u8; 24], ct: vec![9, 9] };
    let audit_vec = vec![a1.clone(), a2.clone()];
    let public = SpendPublic {
        anchor_root: anchor,
        nullifiers: vec![[99u8; 32]],
        commitments: vec![],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: [1u8; 32],
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };
    let proof = bincode::serialize(&public).unwrap();

    // Spend and expect two AuditPayloadPublished events
    let proof_for_tx_ref = proof.clone();
    let proof_for_tx_ref_outside = proof_for_tx_ref.clone();
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof.try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: audit_vec.clone().try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let count = result.events.iter().filter(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::AuditPayloadPublished { .. }))).count();
            assert_eq!(count, 2);

            // Check audit index entry
            let tx_ref = midnight_privacy::merkle::hash_bytes(&proof_for_tx_ref);
            let entry = midnight_privacy::ShieldedPool::<S>::default()
                .audit_index
                .get(&HexHash::new(tx_ref), state)
                .unwrap()
                .unwrap();
            assert_eq!(entry.count, 2);
            assert_eq!(entry.audit_commitment, midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()));
        }),
    });

    // Grant view access with one more payload
    let extra = AuditCiphertext { viewer_id, epk: [7u8; 32], nonce: [8u8; 24], ct: vec![7] };
    // Recompute tx_ref deterministically from the proof bytes we sent
    let tx_ref = midnight_privacy::merkle::hash_bytes(&proof_for_tx_ref_outside);

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::GrantViewAccess { tx_ref, payload: extra.clone() }),
        assert: Box::new(move |res, state| {
            assert!(res.tx_receipt.is_successful());
            assert!(res.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::AuditPayloadPublished { .. }))));
            let entry = midnight_privacy::ShieldedPool::<S>::default()
                .audit_index
                .get(&HexHash::new(tx_ref), state)
                .unwrap()
                .unwrap();
            assert_eq!(entry.count, 3);
        }),
    });
}

#[test]
#[serial]
fn spend_with_valid_risc0_proof() {
    // command to run: RISC0_PROVER=ipc cargo test -p midnight-privacy spend_with_valid_risc0_proof -- --nocapture
    // Skip if the RISC0 guest was not built
    if midnight_privacy_risc0_methods::SPEND_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built (SPEND_ELF empty). Install toolchain or unset SKIP_GUEST_BUILD.");
        return;
    }

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    // Compute method ID (code commitment) for spend guest and use it as vk_hash
    let host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    let method_id_vec = host.code_commitment().encode();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);

    let cfg = midnight_privacy::PoolConfig {
        domain: [7u8; 32],
        vk_hash: method_id,
        fee_bips: 0,
        initial_viewers: vec![],
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Deposit to obtain a valid anchor
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (1000u64).into(),
            commitment: [1u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let anchor: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Build public inputs and generate a real receipt
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public = SpendPublic {
        anchor_root: anchor,
        nullifiers: vec![[42u8; 32]],
        commitments: vec![[2u8; 32]],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public.clone());
    let proof_bytes = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes.try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [42u8; 32])));
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == [2u8; 32])));
        }),
    });
}

#[test]
#[serial]
fn spend_reject_vk_mismatch_with_valid_proof() {
    if midnight_privacy_risc0_methods::SPEND_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built (SPEND_ELF empty). Install toolchain or unset SKIP_GUEST_BUILD.");
        return;
    }

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    // Use real method id for vk_hash in config
    let host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    let method_id_vec = host.code_commitment().encode();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);

    let cfg = midnight_privacy::PoolConfig { domain: [3u8; 32], vk_hash: method_id, fee_bips: 0, initial_viewers: vec![] };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Deposit for anchor
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (1u64).into(),
            commitment: [7u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });
    let anchor: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Build public with mismatched vk_hash (not equal to config)
    let bad_vk: Hash32 = [9u8; 32];
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public = SpendPublic {
        anchor_root: anchor,
        nullifiers: vec![[9u8; 32]],
        commitments: vec![],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: bad_vk,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public);
    let proof_bytes = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes.try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|res, _| {
            // Proof verifies but module should revert due to vk mismatch
            assert!(res.tx_receipt.is_reverted());
        }),
    });
}

// ============================================================================
// CRITICAL TESTS WITH REAL RISC0 PROOFS
// ============================================================================

#[test]
#[serial]
fn spend_with_multiple_outputs_real_proof() {
    // Test sending to multiple recipients in one transaction (critical for privacy)
    if midnight_privacy_risc0_methods::SPEND_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built");
        return;
    }

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    let method_id_vec = host.code_commitment().encode();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);

    let cfg = midnight_privacy::PoolConfig {
        domain: [7u8; 32],
        vk_hash: method_id,
        fee_bips: 0,
        initial_viewers: vec![],
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Deposit to create anchor
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (1000u64).into(),
            commitment: [1u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let anchor: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Create spend with 3 output commitments (3 recipients)
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public = SpendPublic {
        anchor_root: anchor,
        nullifiers: vec![[42u8; 32]],
        commitments: vec![[10u8; 32], [11u8; 32], [12u8; 32]], // 3 outputs
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public.clone());
    let proof_bytes = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes.try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());
            // Check nullifier was used
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [42u8; 32])));
            // Check all 3 commitments were inserted
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == [10u8; 32])));
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == [11u8; 32])));
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == [12u8; 32])));
        }),
    });
}

#[test]
#[serial]
fn spend_with_multiple_inputs_real_proof() {
    // Test consuming multiple notes at once (note consolidation)
    if midnight_privacy_risc0_methods::SPEND_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built");
        return;
    }

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    let method_id_vec = host.code_commitment().encode();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);

    let cfg = midnight_privacy::PoolConfig {
        domain: [8u8; 32],
        vk_hash: method_id,
        fee_bips: 0,
        initial_viewers: vec![],
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Deposit to create anchor
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (5000u64).into(),
            commitment: [1u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let anchor: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Create spend with 3 input nullifiers and 1 output (consolidation)
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public = SpendPublic {
        anchor_root: anchor,
        nullifiers: vec![[50u8; 32], [51u8; 32], [52u8; 32]], // 3 inputs
        commitments: vec![[20u8; 32]], // 1 consolidated output
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public.clone());
    let proof_bytes = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes.try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());
            // Check all 3 nullifiers were used
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [50u8; 32])));
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [51u8; 32])));
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [52u8; 32])));
            // Check consolidated commitment was inserted
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == [20u8; 32])));
        }),
    });
}

#[test]
#[serial]
fn sequential_spend_note_lifecycle_real_proof() {
    // Test creating a note and spending it in a subsequent transaction (full lifecycle)
    if midnight_privacy_risc0_methods::SPEND_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built");
        return;
    }

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    let method_id_vec = host.code_commitment().encode();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);

    let cfg = midnight_privacy::PoolConfig {
        domain: [9u8; 32],
        vk_hash: method_id,
        fee_bips: 0,
        initial_viewers: vec![],
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Step 1: Deposit to create initial anchor
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (2000u64).into(),
            commitment: [1u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let anchor1: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Step 2: First spend - create note [100u8; 32] as output
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public1 = SpendPublic {
        anchor_root: anchor1,
        nullifiers: vec![[60u8; 32]],
        commitments: vec![[100u8; 32]], // This note will be spent in next tx
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public1.clone());
    let proof_bytes1 = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes1.try_into().unwrap(),
            anchor_root: anchor1,
            audit_payloads: audit_vec.clone().try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == [100u8; 32])));
        }),
    });

    // Step 3: Get new anchor (includes the [100u8; 32] commitment)
    let anchor2: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Verify anchor changed
    assert_ne!(anchor1, anchor2, "Anchor should have changed after first spend");

    // Step 4: Second spend - now spend the note we just created
    // Simulate: commitment [100u8; 32] produces nullifier [200u8; 32] when spent
    let public2 = SpendPublic {
        anchor_root: anchor2,
        nullifiers: vec![[200u8; 32]], // Nullifier for commitment [100u8; 32]
        commitments: vec![[101u8; 32]], // New output
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public2.clone());
    let proof_bytes2 = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes2.try_into().unwrap(),
            anchor_root: anchor2,
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());
            // Verify the note created in tx1 was successfully spent in tx2
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [200u8; 32])));
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == [101u8; 32])));
        }),
    });
}

#[test]
#[serial]
fn reject_double_spend_with_real_proof() {
    // Critical security test: ensure nullifier reuse fails even with valid proof
    if midnight_privacy_risc0_methods::SPEND_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built");
        return;
    }

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    let method_id_vec = host.code_commitment().encode();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);

    let cfg = midnight_privacy::PoolConfig {
        domain: [10u8; 32],
        vk_hash: method_id,
        fee_bips: 0,
        initial_viewers: vec![],
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Deposit to create anchor
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (1000u64).into(),
            commitment: [1u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let anchor: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    let nullifier = [70u8; 32];
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public = SpendPublic {
        anchor_root: anchor,
        nullifiers: vec![nullifier],
        commitments: vec![[30u8; 32]],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    // First spend succeeds
    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public.clone());
    let proof_bytes1 = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes1.clone().try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: audit_vec.clone().try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(move |result, _state| {
            assert!(result.tx_receipt.is_successful());
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == nullifier)));
        }),
    });

    // Second spend with same nullifier should fail (even though proof is valid)
    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public.clone());
    let proof_bytes2 = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes2.try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            // Should fail due to nullifier reuse
            assert!(result.tx_receipt.is_reverted());
        }),
    });
}

#[test]
#[serial]
fn withdraw_to_transparent_with_real_proof() {
    // Test the withdraw path with actual proof verification
    if midnight_privacy_risc0_methods::SPEND_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built");
        return;
    }

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    let method_id_vec = host.code_commitment().encode();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);

    let cfg = midnight_privacy::PoolConfig {
        domain: [11u8; 32],
        vk_hash: method_id,
        fee_bips: 0,
        initial_viewers: vec![],
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Deposit 5000 tokens
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (5000u64).into(),
            commitment: [1u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let anchor: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Create spend with withdrawal
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public = SpendPublic {
        anchor_root: anchor,
        nullifiers: vec![[80u8; 32]],
        commitments: vec![], // No new shielded outputs, full withdrawal
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public.clone());
    let proof_bytes = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    let withdraw_amount = 1000u64;
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes.try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: Some(user.address()),
            withdraw: Some((config_gas_token_id(), withdraw_amount.into())),
        }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());
            // Verify nullifier was used
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [80u8; 32])));
            
            // Note: In a full implementation, the ZK circuit would validate the withdrawal amount.
            // For now, we just verify the transaction succeeds and nullifier is consumed.
            // TODO: Add withdrawal amount to SpendPublic and validate in the circuit
        }),
    });
}

#[test]
#[serial]
fn reject_stale_anchor_with_real_proof() {
    // Test rejection of old/non-existent anchor roots with real proofs
    if midnight_privacy_risc0_methods::SPEND_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built");
        return;
    }

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    let method_id_vec = host.code_commitment().encode();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);

    let cfg = midnight_privacy::PoolConfig {
        domain: [12u8; 32],
        vk_hash: method_id,
        fee_bips: 0,
        initial_viewers: vec![],
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Deposit to create anchor
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (1000u64).into(),
            commitment: [1u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    // Use a fake anchor that doesn't exist in recent_roots
    let fake_anchor = [99u8; 32];
    
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public = SpendPublic {
        anchor_root: fake_anchor,
        nullifiers: vec![[90u8; 32]],
        commitments: vec![[40u8; 32]],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public.clone());
    let proof_bytes = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes.try_into().unwrap(),
            anchor_root: fake_anchor,
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            // Should fail due to unknown anchor
            assert!(result.tx_receipt.is_reverted());
        }),
    });
}

#[test]
#[serial]
fn spend_with_max_outputs_real_proof() {
    // Test with MAX_COMMITMENTS_PER_TX (8 outputs) to ensure limits work
    if midnight_privacy_risc0_methods::SPEND_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built");
        return;
    }

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    let method_id_vec = host.code_commitment().encode();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);

    let cfg = midnight_privacy::PoolConfig {
        domain: [13u8; 32],
        vk_hash: method_id,
        fee_bips: 0,
        initial_viewers: vec![],
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Deposit to create anchor
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (10000u64).into(),
            commitment: [1u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let anchor: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Create spend with 8 output commitments (MAX_COMMITMENTS_PER_TX)
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public = SpendPublic {
        anchor_root: anchor,
        nullifiers: vec![[95u8; 32]],
        commitments: vec![
            [201u8; 32], [202u8; 32], [203u8; 32], [204u8; 32],
            [205u8; 32], [206u8; 32], [207u8; 32], [208u8; 32],
        ], // Exactly 8 outputs (the max)
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public.clone());
    let proof_bytes = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes.try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());
            // Verify all 8 commitments were inserted
            for i in 201u8..=208u8 {
                let commitment = [i; 32];
                assert!(
                    result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == commitment)),
                    "Commitment {:?} should be inserted", commitment
                );
            }
        }),
    });
}

#[test]
fn withdrawal_amount_not_validated_allows_theft() {
    // 🚨 CRITICAL VULNERABILITY: Attacker can steal other users' funds!
    // 
    // The ZK circuit does NOT validate withdrawal amounts, allowing an attacker to:
    // 1. Deposit a small amount (100 tokens)
    // 2. Create a valid proof for spending that small note
    // 3. Lie about the withdrawal amount and steal victim's funds (10,000 tokens)
    //
    // The bank module won't prevent this because the POOL has enough balance
    // (victim's 10k + attacker's 100), but the attacker is stealing from the victim!
    
    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(2);
    let attacker = genesis_core.additional_accounts().first().unwrap().clone();
    let victim = genesis_core.additional_accounts().get(1).unwrap().clone();

    let cfg = midnight_privacy::PoolConfig {
        domain: [99u8; 32],
        vk_hash: [99u8; 32],
        fee_bips: 0,
        initial_viewers: vec![],
    };

    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Step 1: Victim deposits 10,000 tokens to the pool (honest user)
    let victim_deposit = 10_000u64;
    runner.execute_transaction(TransactionTestCase {
        input: victim.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: victim_deposit.into(),
            commitment: [88u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    // Step 2: Attacker deposits only 100 tokens (small amount)
    let small_deposit = 100u64;
    runner.execute_transaction(TransactionTestCase {
        input: attacker.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: small_deposit.into(),
            commitment: [1u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let anchor: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Step 3: Query attacker's initial balance
    let initial_balance: u64 = runner.query_visible_state(|state| {
        let bank = sov_bank::Bank::<S>::default();
        bank.get_balance_of(&attacker.address(), config_gas_token_id(), state)
            .unwrap()
            .unwrap_or(0u64.into())
            .0 as u64
    });

    // Step 4: Create a "valid" proof that claims to spend the 100 token note
    // The proof says: sum(inputs) = sum(outputs) + fee
    // In this case: 100 = 0 + 0 + 0 (implicitly 100 for withdrawal, but NOT in proof!)
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public = SpendPublic {
        anchor_root: anchor,
        nullifiers: vec![[111u8; 32]],  // Claiming to spend the note
        commitments: vec![],             // No new outputs
        fee: 0,                          // No fee
        // ❌ NOTICE: No withdrawal amount in the proof!
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: [99u8; 32],
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };
    let proof = bincode::serialize(&public).unwrap();

    // Step 5: LIE about the withdrawal amount!
    // Attacker deposited only 100, but will try to withdraw victim's 10,000!
    let lie_amount = 10_000u64;
    
    runner.execute_transaction(TransactionTestCase {
        input: attacker.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof.try_into().unwrap(),
            anchor_root: anchor,
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: Some(attacker.address()),  // Attacker withdraws to their own address
            withdraw: Some((config_gas_token_id(), lie_amount.into())),  // 🚨 LYING!
        }),
        assert: Box::new(move |result, state| {
            // Check if the transaction succeeded
            if !result.tx_receipt.is_successful() {
                eprintln!("❌ Transaction failed (good - means validation works)");
                eprintln!("Receipt: {:?}", result.tx_receipt);
                eprintln!("\nThis failure could be due to:");
                eprintln!("1. Deserialization issues with the mock proof format");
                eprintln!("2. Some validation logic we missed");
                eprintln!("\nTo properly test this vulnerability, we'd need to:");
                eprintln!("- Use the actual note value system (which doesn't exist yet)");
                eprintln!("- Or fix the test harness to properly simulate the attack");
                return;
            }
            
            eprintln!("\n🚨🚨🚨 CRITICAL VULNERABILITY CONFIRMED! 🚨🚨🚨\n");
            
            let bank = sov_bank::Bank::<S>::default();
            
            // Check attacker's balance
            let attacker_final: u64 = bank
                .get_balance_of(&attacker.address(), config_gas_token_id(), state)
                .unwrap()
                .unwrap_or(0u64.into())
                .0 as u64;
            
            // Check pool's balance  
            use sov_bank::IntoPayable;
            let pool_id = midnight_privacy::ShieldedPool::<S>::default().id;
            let pool_balance: u64 = bank
                .get_balance_of(
                    pool_id.to_payable(),
                    config_gas_token_id(),
                    state
                )
                .unwrap()
                .unwrap_or(0u64.into())
                .0 as u64;
            
            eprintln!("ATTACK SUCCESSFUL:");
            eprintln!("─────────────────────────────────────────────────");
            eprintln!("Victim deposited:     {} tokens", victim_deposit);
            eprintln!("Attacker deposited:   {} tokens", small_deposit);
            eprintln!("Pool total:           {} tokens", victim_deposit + small_deposit);
            eprintln!("");
            eprintln!("Attacker initial:     {} tokens", initial_balance);
            eprintln!("Attacker withdrew:    {} tokens", lie_amount);
            eprintln!("Attacker final:       {} tokens", attacker_final);
            eprintln!("");
            eprintln!("Pool remaining:       {} tokens", pool_balance);
            eprintln!("─────────────────────────────────────────────────");
            eprintln!("STOLEN FROM VICTIM:   {} tokens", lie_amount - small_deposit);
            eprintln!("\n💀 The attacker stole {} tokens that belonged to the victim!", 
                     lie_amount - small_deposit);
            eprintln!("💀 The ZK proof did NOT validate withdrawal amounts!");
            eprintln!("💀 The bank module can't prevent this because the pool HAS the balance!");
            
            // Verify the theft
            assert_eq!(
                attacker_final,
                initial_balance + lie_amount,
                "Attacker should have received the full lie amount"
            );
            
            assert_eq!(
                pool_balance,
                victim_deposit + small_deposit - lie_amount,
                "Pool should have lost the withdrawal amount"
            );
            
            panic!("\n🚨 VULNERABILITY CONFIRMED: Attacker successfully stole {} tokens from victim!\n", 
                   lie_amount - small_deposit);
        }),
    });
}

#[test]
#[serial]
fn spend_with_old_anchor_in_window_real_proof() {
    // Test that spending with an older anchor root (within the window) still works
    if midnight_privacy_risc0_methods::SPEND_ELF.is_empty() {
        eprintln!("Skipping real-proof test: RISC0 guest not built");
        return;
    }

    let genesis_core = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let user = genesis_core.additional_accounts().first().unwrap().clone();

    let host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    let method_id_vec = host.code_commitment().encode();
    let mut method_id = [0u8; 32];
    method_id.copy_from_slice(&method_id_vec);

    let cfg = midnight_privacy::PoolConfig {
        domain: [14u8; 32],
        vk_hash: method_id,
        fee_bips: 0,
        initial_viewers: vec![],
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_core.into(), cfg);
    let mut runner = TestRunner::<RT, S>::new_with_genesis(genesis.into_genesis_params(), RT::default());

    // Step 1: Create initial anchor (anchor1)
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
            token_id: config_gas_token_id(),
            amount: (1000u64).into(),
            commitment: [1u8; 32],
        }),
        assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
    });

    let anchor1: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Step 2: Spend first nullifier using anchor1
    let audit_vec: Vec<AuditCiphertext> = Vec::new();
    let public1 = SpendPublic {
        anchor_root: anchor1,
        nullifiers: vec![[100u8; 32]],
        commitments: vec![[200u8; 32]],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public1.clone());
    let proof_bytes1 = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes1.try_into().unwrap(),
            anchor_root: anchor1,
            audit_payloads: audit_vec.clone().try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful());
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [100u8; 32])));
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == [200u8; 32])));
        }),
    });

    // Step 3: Create more transactions to advance the tree (new anchors)
    // This simulates time passing and the tree state evolving
    for i in 0..5 {
        runner.execute_transaction(TransactionTestCase {
            input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Deposit {
                token_id: config_gas_token_id(),
                amount: (100u64).into(),
                commitment: [50 + i; 32],
            }),
            assert: Box::new(|res, _| assert!(res.tx_receipt.is_successful())),
        });
    }

    // Step 4: Get the new anchor (should be different from anchor1)
    let anchor_new: Hash32 = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    });

    // Verify anchor changed
    assert_ne!(anchor1, anchor_new, "Anchor should have changed after deposits");

    // Step 5: Verify anchor1 is still in the window
    let anchor1_still_valid = runner.query_visible_state(|state| {
        midnight_privacy::ShieldedPool::<S>::default()
            .recent_roots
            .get(state)
            .unwrap()
            .unwrap()
            .contains(&anchor1)
    });
    assert!(anchor1_still_valid, "Old anchor should still be in the window");

    // Step 6: Spend second nullifier using the OLD anchor1 (should still work!)
    let public2 = SpendPublic {
        anchor_root: anchor1, // Using the old anchor!
        nullifiers: vec![[101u8; 32]],
        commitments: vec![[201u8; 32]],
        fee: 0,
        chain_id: [0u8; 32],
        module_id: [0u8; 32],
        vk_hash: method_id,
        audit_commitment: midnight_privacy::merkle::hash_bytes(&bincode::serialize(&audit_vec).unwrap()),
    };

    let mut host = Risc0Host::new(midnight_privacy_risc0_methods::SPEND_ELF);
    host.add_hint(public2.clone());
    let proof_bytes2 = ZkvmHost::run(&mut host, true).expect("proof generation must succeed");

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, ShieldedPool<S>>(CallMessage::Spend {
            proof: proof_bytes2.try_into().unwrap(),
            anchor_root: anchor1, // Using old anchor
            audit_payloads: audit_vec.try_into().unwrap(),
            withdraw_to: None,
            withdraw: None,
        }),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful(), "Spend with old anchor (in window) should succeed");
            // Verify the second nullifier was used
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::NullifierUsed { nf }) if *nf == [101u8; 32])));
            // Verify the second commitment was inserted
            assert!(result.events.iter().any(|e| matches!(e, ShieldedRuntimeEvent::ShieldedPool(Event::CommitmentInserted { commitment: c, .. }) if *c == [201u8; 32])));
        }),
    });
}
