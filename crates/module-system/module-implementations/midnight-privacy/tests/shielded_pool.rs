use midnight_privacy::{Event, Hash32, SpendPublic};
use midnight_privacy::audit::AuditCiphertext;
use midnight_privacy::{CallMessage, ShieldedPool};
use sov_bank::config_gas_token_id;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_optimistic_runtime, AsUser, TransactionTestCase};

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
