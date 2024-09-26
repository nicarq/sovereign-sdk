#![allow(clippy::float_arithmetic)]

use std::env;

use criterion::{criterion_group, criterion_main, Criterion};
use demo_stf::genesis_config::EvmConfig;
use demo_stf::runtime::{GenesisConfig, Runtime};
use sov_bank::{Bank, Coins, TokenId};
use sov_db::storage_manager::NativeChangeSet;
use sov_kernels::basic::BasicKernel;
use sov_mock_da::{MockAddress, MockBlob, MockBlock, MockDaSpec, MOCK_SEQUENCER_DA_ADDRESS};
use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
use sov_modules_api::capabilities::TransactionAuthenticator;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{
    Batch, BatchSequencerOutcome, BatchSequencerReceipt, EncodeCall, FullyBakedTx, Gas, GasSpec,
    GasUnit, OperatingMode, RawTx, Spec,
};
use sov_modules_macros::config_value;
use sov_modules_stf_blueprint::{GenesisParams, StfBlueprint, TxReceiptContents};
use sov_nft::NonFungibleTokenConfig;
use sov_rollup_interface::crypto::PrivateKey;
use sov_rollup_interface::da::RelevantBlobs;
use sov_rollup_interface::stf::{ExecutionContext, StateTransitionFunction};
use sov_state::{ProverStorage, StorageRoot};
use sov_test_utils::runtime::genesis::default_basic_kernel_genesis;
use sov_test_utils::runtime::genesis::zk::MinimalZkGenesisConfig;
use sov_test_utils::storage::SimpleStorageManager;
use sov_test_utils::{
    TestPrivateKey, TestProver, TestSequencer, TestSpec, TestStorageSpec, TestUser,
    TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE, TEST_DEFAULT_USER_STAKE,
};
use sov_value_setter::ValueSetterConfig;
use tempfile::TempDir;

type BenchSpec = sov_test_utils::TestSpec;
type Stf = StfBlueprint<
    BenchSpec,
    MockDaSpec,
    Runtime<BenchSpec, MockDaSpec>,
    BasicKernel<BenchSpec, MockDaSpec>,
>;

type BatchReceipt<S> = sov_rollup_interface::stf::BatchReceipt<
    BatchSequencerReceipt<MockDaSpec>,
    TxReceiptContents<S>,
    <<S as Spec>::Gas as Gas>::Price,
>;

const CHAIN_ID: u64 = config_value!("CHAIN_ID");
const DEFAULT_ESTIMATED_GAS_USAGE: Option<GasUnit<2>> = None;

fn bake_bank_tx(
    msg: sov_bank::CallMessage<BenchSpec>,
    pk: &Ed25519PrivateKey,
    nonce: u64,
) -> FullyBakedTx {
    let enc_msg = <Runtime<BenchSpec, MockDaSpec> as EncodeCall<Bank<BenchSpec>>>::encode_call(msg);

    let tx = Transaction::<BenchSpec>::new_signed_tx(
        pk,
        UnsignedTransaction::new(
            enc_msg,
            CHAIN_ID,
            TEST_DEFAULT_MAX_PRIORITY_FEE,
            TEST_DEFAULT_MAX_FEE,
            nonce,
            DEFAULT_ESTIMATED_GAS_USAGE,
        ),
    );
    <Runtime<BenchSpec, MockDaSpec> as TransactionAuthenticator<BenchSpec>>::encode_with_standard_auth(
        RawTx::new(borsh::to_vec(&tx).unwrap()),
    )
}

fn build_batch_blob(txs: Vec<FullyBakedTx>) -> RelevantBlobs<MockBlob> {
    let blob = borsh::to_vec(&Batch::new(txs)).unwrap();

    let address = MockAddress::from(MOCK_SEQUENCER_DA_ADDRESS);
    let blob = MockBlob::new_with_hash(blob, address);

    RelevantBlobs {
        proof_blobs: Default::default(),
        batch_blobs: vec![blob],
    }
}

fn build_send_tx(sender: &TestUser<BenchSpec>, nonce: u64, token_id: TokenId) -> FullyBakedTx {
    let priv_key = TestPrivateKey::generate();
    let to_address: <BenchSpec as Spec>::Address = (&priv_key.pub_key()).into();
    bake_bank_tx(
        sov_bank::CallMessage::<BenchSpec>::Transfer {
            to: to_address,
            coins: Coins {
                amount: 1,
                token_id,
            },
        },
        &sender.private_key,
        nonce,
    )
}

fn assert_batch_receipts<S: Spec>(batch_receipts: &[BatchReceipt<S>]) {
    for batch in batch_receipts {
        if let BatchSequencerOutcome::Rewarded(r) = batch.inner.outcome {
            assert_eq!(0, r.0);
        } else {
            panic!("Unexpected batch outcome: {:?}", batch.inner.outcome);
        }
        for tx in &batch.tx_receipts {
            assert!(
                tx.receipt.is_successful(),
                "Non successful tx: {:?}",
                tx.receipt
            );
        }
    }
}

fn initialize_rollup(
    minimal_config: MinimalZkGenesisConfig<BenchSpec, MockDaSpec>,
    stf: &Stf,
    stf_state: ProverStorage<TestStorageSpec>,
) -> (sov_state::StorageRoot<TestStorageSpec>, NativeChangeSet) {
    let MinimalZkGenesisConfig {
        sequencer_registry,
        prover_incentives,
        attester_incentives,
        bank,
        accounts,
        nonces,
    } = minimal_config;

    let rt_genesis = GenesisConfig::new(
        bank,
        sequencer_registry,
        ValueSetterConfig::<BenchSpec> {
            // Not important for this benchmark
            admin: TestUser::<BenchSpec>::generate_with_default_balance().address(),
        },
        attester_incentives,
        prover_incentives,
        accounts,
        nonces,
        NonFungibleTokenConfig {},
        EvmConfig::default(),
    );
    let kernel_genesis = default_basic_kernel_genesis(OperatingMode::Zk);
    let genesis_config = GenesisParams {
        runtime: rt_genesis,
        kernel: kernel_genesis,
    };

    stf.init_chain(stf_state, genesis_config)
}

fn prefill_state(
    storage_manager: &mut SimpleStorageManager<TestStorageSpec>,
    mut current_root: StorageRoot<TestStorageSpec>,
    stf: &Stf,
    rollup_mega_admin: &TestUser<BenchSpec>,
    senders: &[TestUser<BenchSpec>],
    blocks_to_process: u64,
) -> (StorageRoot<TestStorageSpec>, TokenId) {
    let token_name = "sov-bench-token";
    let salt = u64::MAX;
    let token_id =
        sov_bank::get_token_id::<BenchSpec>(token_name, &rollup_mega_admin.address(), salt);

    let create_token_msg = bake_bank_tx(
        sov_bank::CallMessage::<BenchSpec>::CreateToken {
            salt,
            token_name: token_name.to_string(),
            initial_balance: 0,
            mint_to_address: rollup_mega_admin.address(),
            authorized_minters: vec![rollup_mega_admin.address()],
        },
        &rollup_mega_admin.private_key,
        0,
    );

    let coins_per_sender = u64::MAX / senders.len() as u64;

    let init_messages: Vec<_> = std::iter::once(create_token_msg)
        .chain(senders.iter().enumerate().map(|(idx, sender)| {
            bake_bank_tx(
                sov_bank::CallMessage::<BenchSpec>::Mint {
                    coins: Coins {
                        amount: coins_per_sender,
                        token_id,
                    },
                    mint_to_address: sender.address(),
                },
                &rollup_mega_admin.private_key,
                (idx + 1) as u64,
            )
        }))
        .collect();

    let stf_state = storage_manager.create_storage();
    let filtered_block = MockBlock::default_at_height(1);
    let mut blobs = build_batch_blob(init_messages);
    let apply_slot_output = stf.apply_slot(
        &current_root,
        stf_state,
        Default::default(),
        &filtered_block.header,
        &filtered_block.validity_cond,
        blobs.as_iters(),
        ExecutionContext::Node,
    );
    current_root = apply_slot_output.state_root;
    storage_manager.commit(apply_slot_output.change_set);
    assert_batch_receipts(&apply_slot_output.batch_receipts);

    for i in 0..blocks_to_process {
        let stf_state = storage_manager.create_storage();
        let filtered_block = MockBlock::default_at_height(i + 1);
        let send_messages = senders
            .iter()
            .map(|sender| build_send_tx(sender, i, token_id))
            .collect::<Vec<_>>();
        let mut blobs = build_batch_blob(send_messages);
        let apply_slot_output = stf.apply_slot(
            &current_root,
            stf_state,
            Default::default(),
            &filtered_block.header,
            &filtered_block.validity_cond,
            blobs.as_iters(),
            ExecutionContext::Node,
        );
        assert_batch_receipts(&apply_slot_output.batch_receipts);
        current_root = apply_slot_output.state_root;
        storage_manager.commit(apply_slot_output.change_set);
    }

    (current_root, token_id)
}

fn stf_apply_slot_bench(c: &mut Criterion) {
    let bench_after_blocks: u64 = env::var("BLOCKS")
        .unwrap_or("100".to_string())
        .parse()
        .expect("BLOCKS var should be a positive number");
    let senders_count = env::var("TXNS_PER_BLOCK")
        .unwrap_or("1000".to_string())
        .parse()
        .expect("TXS_PER_BLOCK var should be a positive number");

    println!(
        "Going to bench after {} blocks, with {} unique senders.",
        bench_after_blocks, senders_count
    );
    println!("Each block will have sov_bank::Bank::Transfer call message from each sender to random address.");
    println!(
        "Meaning that when bench start there will be {} transfers in a tree plus minting for each sender in the beginning.",
        bench_after_blocks * senders_count
    );

    let temp_dir = TempDir::new().expect("Unable to create temporary directory");
    let mut storage_manager = SimpleStorageManager::new(temp_dir.path());
    let stf = Stf::new();

    let rollup_mega_admin = TestUser::generate_with_default_balance();
    let senders: Vec<_> = (0..senders_count)
        .map(|_| TestUser::generate_with_default_balance())
        .collect();

    let user_stake = <TestSpec as Spec>::Gas::from(TEST_DEFAULT_USER_STAKE);
    let user_stake_value = user_stake.value(&TestSpec::initial_base_fee_per_gas());

    let minimal_config = MinimalZkGenesisConfig::<BenchSpec, MockDaSpec>::from_args(
        TestProver {
            user_info: rollup_mega_admin.clone(),
            bond: user_stake_value,
        },
        TestSequencer {
            user_info: rollup_mega_admin.clone(),
            da_address: MOCK_SEQUENCER_DA_ADDRESS.into(),
            bond: user_stake_value,
        },
        &senders,
        "sov-test-gas-token".to_string(),
    );

    let (current_root, stf_change_set) =
        initialize_rollup(minimal_config, &stf, storage_manager.create_storage());
    storage_manager.commit(stf_change_set);

    let (current_root, token_id) = prefill_state(
        &mut storage_manager,
        current_root,
        &stf,
        &rollup_mega_admin,
        &senders,
        bench_after_blocks,
    );

    let bench_messages = senders
        .iter()
        .map(|sender| build_send_tx(sender, bench_after_blocks, token_id))
        .collect::<Vec<_>>();

    c.bench_function("rollup main stf loop", |b| {
        b.iter(|| {
            let stf_state = storage_manager.create_storage();
            let filtered_block = MockBlock::default_at_height(bench_after_blocks + 1);
            let mut blobs = build_batch_blob(bench_messages.clone());
            let apply_slot_output = stf.apply_slot(
                &current_root,
                stf_state,
                Default::default(),
                &filtered_block.header,
                &filtered_block.validity_cond,
                blobs.as_iters(),
                ExecutionContext::Node,
            );
            assert_batch_receipts(&apply_slot_output.batch_receipts);
        });
    });
}

fn configure_criterion() -> Criterion {
    Criterion::default()
        .warm_up_time(std::time::Duration::from_millis(20))
        .measurement_time(std::time::Duration::from_secs(80))
}

criterion_group! {
    name = benches;
    config = configure_criterion();
    targets = stf_apply_slot_bench
}
criterion_main!(benches);
