#![allow(clippy::float_arithmetic)]

#[macro_use]
extern crate prettytable;

use std::default::Default;
use std::env;
use std::path::Path;
use std::time::{Duration, Instant};

use demo_stf::authentication::ModAuth;
use demo_stf::genesis_config::{create_genesis_config, GenesisPaths};
use demo_stf::runtime::Runtime;
use humantime::format_duration;
use prettytable::Table;
use sov_bank::{Bank, Coins};
use sov_db::ledger_db::{LedgerDb, SlotCommit};
use sov_db::storage_manager::NativeStorageManager;
use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockAddress, MockBlob, MockBlock, MockBlockHeader, MockDaSpec};
use sov_modules_api::capabilities::Authenticator;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{Batch, BatchSequencerOutcome, EncodeCall, RawTx, Spec};
use sov_modules_stf_blueprint::{GenesisParams, StfBlueprint};
use sov_rollup_interface::crypto::{PrivateKey, PublicKey};
use sov_rollup_interface::da::{BlockHeaderTrait, RelevantBlobs};
use sov_rollup_interface::node::da::SlotData;
use sov_rollup_interface::stf::{ExecutionContext, StateTransitionFunction};
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::CryptoSpec;
use sov_state::StorageRoot;
use sov_test_utils::{TestPrivateKey, TestSpec, TestStorageManager, TestStorageSpec};
use tempfile::TempDir;

type BenchStf = StfBlueprint<
    TestSpec,
    MockDaSpec,
    Runtime<TestSpec, MockDaSpec>,
    BasicKernel<TestSpec, MockDaSpec>,
>;

const SEQUENCER_ADDRESS: MockAddress = MockAddress::new([0; 32]);
// Minimum TPS below which it is considered an issue
const MIN_TPS: f64 = 1000.0;
// Number to check that rollup actually executed some transactions
const MAX_TPS: f64 = 1_000_000.0;

fn print_times(
    total: Duration,
    apply_block_time: Duration,
    blocks: u64,
    num_txns: u64,
    num_success_txns: u64,
) {
    let mut table = Table::new();

    let total_txns = blocks * num_txns;
    table.add_row(row!["Blocks", format!("{:?}", blocks)]);
    table.add_row(row!["Transactions per block", format!("{:?}", num_txns)]);
    table.add_row(row![
        "Processed transactions (success/total)",
        format!("{:?}/{:?}", num_success_txns, total_txns)
    ]);
    table.add_row(row!["Total", format_duration(total)]);
    table.add_row(row!["Apply block", format_duration(apply_block_time)]);
    let tps = (total_txns as f64) / total.as_secs_f64();
    table.add_row(row!["Transactions per sec (TPS)", format!("{:.1}", tps)]);

    // Print the table to stdout
    table.printstd();

    assert!(
        tps > MIN_TPS,
        "TPS {} dropped below {}, investigation is needed",
        tps,
        MIN_TPS
    );
    assert!(
        tps < MAX_TPS,
        "TPS {} reached unrealistic number {}, investigation is needed",
        tps,
        MAX_TPS
    );
}

#[derive(Debug)]
struct BenchParams {
    blocks: u64,
    transactions_per_block: u64,
    timer_output: bool,
}

impl BenchParams {
    fn new() -> Self {
        let mut blocks: u64 = 10;
        let mut transactions_per_block = 10000;
        let mut timer_output = true;

        if let Ok(val) = env::var("TXNS_PER_BLOCK") {
            transactions_per_block = val
                .parse()
                .expect("TXNS_PER_BLOCK var should be a +ve number");
        }
        if let Ok(val) = env::var("BLOCKS") {
            blocks = val
                .parse::<u64>()
                .expect("BLOCKS var should be a positive integer");
        }
        if let Ok(val) = env::var("TIMER_OUTPUT") {
            match val.as_str() {
                "true" | "1" | "yes" => {
                    timer_output = true;
                }
                "false" | "0" | "no" => (),
                val => {
                    panic!(
                        "Unknown value '{}' for TIMER_OUTPUT. expected true/false/0/1/yes/no",
                        val
                    );
                }
            }
        }

        Self {
            blocks,
            transactions_per_block,
            timer_output,
        }
    }
}

fn sender_address_with_pkey<S: Spec>() -> (S::Address, TestPrivateKey)
where
    S::Address: From<[u8; 32]>,
{
    let pk = TestPrivateKey::generate();
    let addr = pk
        .pub_key()
        .credential_id::<<S::CryptoSpec as CryptoSpec>::Hasher>()
        .0
         .0
        .into();

    (addr, pk)
}

fn signed_bank_tx(
    msg: sov_bank::CallMessage<TestSpec>,
    private_key: &TestPrivateKey,
    nonce: u64,
) -> RawTx {
    let enc_msg = <Runtime<TestSpec, MockDaSpec> as EncodeCall<Bank<TestSpec>>>::encode_call(msg);
    let tx = Transaction::<TestSpec>::new_signed_tx(
        private_key,
        UnsignedTransaction::new(
            enc_msg,
            sov_modules_api::capabilities::CHAIN_ID,
            sov_test_utils::TEST_DEFAULT_MAX_PRIORITY_FEE,
            sov_test_utils::TEST_DEFAULT_MAX_FEE,
            nonce,
            None,
        ),
    );
    ModAuth::<TestSpec, MockDaSpec>::encode(borsh::to_vec(&tx).unwrap()).unwrap()
}

// Sets up storage and returns blocks that should be benchmarked.
fn setup(
    params: &BenchParams,
    path: impl AsRef<Path>,
) -> (
    TestStorageManager,
    StorageRoot<TestStorageSpec>,
    Vec<MockBlock>,
) {
    let mut storage_manager =
        NativeStorageManager::new(path.as_ref()).expect("StorageManager initialization failed");
    let stf = BenchStf::new();

    // Preparation
    // Each block has its own sender, which sends
    let sender_keys: Vec<_> = (0..params.blocks)
        .map(|_| sender_address_with_pkey::<TestSpec>())
        .collect();
    let (token_deployer_address, token_deployer_key) = sender_keys.first().unwrap();

    // Genesis
    let demo_genesis_config = {
        let stf_tests_conf_dir: &Path = "../test-data/genesis/stf-tests".as_ref();
        let mut rt_params =
            create_genesis_config::<TestSpec, _>(&GenesisPaths::from_dir(stf_tests_conf_dir))
                .unwrap();

        // Funding gas for senders
        let remaining_gas_token_amount = u64::MAX
            - rt_params
                .bank
                .gas_token_config
                .address_and_balances
                .iter()
                .map(|(_, amount)| amount)
                .sum::<u64>();
        let gas_per_sender = remaining_gas_token_amount / sender_keys.len() as u64;

        for (addr, _) in sender_keys.iter() {
            rt_params
                .bank
                .gas_token_config
                .address_and_balances
                .push((*addr, gas_per_sender));
        }

        let kernel_params =
            BasicKernelGenesisConfig::from_path(stf_tests_conf_dir.join("chain_state.json"))
                .unwrap();
        GenesisParams {
            runtime: rt_params,
            kernel: kernel_params,
        }
    };
    let genesis_block_header = MockBlockHeader::from_height(0);
    let (stf_state, ledger_state) = storage_manager
        .create_state_for(&genesis_block_header)
        .expect("Getting genesis storage failed");

    let mut ledger_db = LedgerDb::with_reader(ledger_state).unwrap();

    let (mut current_root, stf_state) = stf.init_chain(stf_state, demo_genesis_config);

    let data_to_commit: SlotCommit<MockBlock, BatchSequencerOutcome, ()> =
        SlotCommit::new(MockBlock {
            header: genesis_block_header.clone(),
            ..Default::default()
        });
    let mut ledger_change_set = ledger_db
        .materialize_slot(data_to_commit, current_root.as_ref())
        .unwrap();
    let finalized_slot_changes = ledger_db.materialize_latest_finalize_slot(0).unwrap();
    ledger_change_set.merge(finalized_slot_changes);

    storage_manager
        .save_change_set(&genesis_block_header, stf_state, ledger_change_set)
        .expect("Saving genesis storage failed");

    let mut setup_txs: Vec<RawTx> = Vec::new();

    let token_name = "sov-bench-token";
    let salt = 31337;
    let token_id = sov_bank::get_token_id::<TestSpec>(token_name, token_deployer_address, salt);
    let msg: sov_bank::CallMessage<TestSpec> = sov_bank::CallMessage::<TestSpec>::CreateToken {
        salt,
        token_name: token_name.to_string(),
        // Mint for everyone, including themselves.
        initial_balance: 0,
        mint_to_address: *token_deployer_address,
        authorized_minters: vec![*token_deployer_address],
    };
    let mut deployer_nonce = 0;
    setup_txs.push(signed_bank_tx(msg, token_deployer_key, deployer_nonce));
    deployer_nonce += 1;
    // Mint for everyone
    let mint_amount = u64::MAX / (sender_keys.len() as u64);
    for (distributor, _) in sender_keys.iter() {
        let msg: sov_bank::CallMessage<TestSpec> = sov_bank::CallMessage::<TestSpec>::Mint {
            coins: Coins {
                amount: mint_amount,
                token_id,
            },
            mint_to_address: *distributor,
        };
        setup_txs.push(signed_bank_tx(msg, token_deployer_key, deployer_nonce));
        deployer_nonce += 1;
    }

    let batch = Batch::new(setup_txs);
    let setup_blob = MockBlob::new_with_hash(borsh::to_vec(&batch).unwrap(), SEQUENCER_ADDRESS);
    let mut setup_blobs = RelevantBlobs::<MockBlob> {
        proof_blobs: Vec::new(),
        batch_blobs: vec![setup_blob.clone()],
    };

    let setup_block_header = MockBlockHeader::from_height(1);
    let (stf_state, ledger_storage) = storage_manager
        .create_state_for(&setup_block_header)
        .unwrap();
    ledger_db.replace_reader(ledger_storage);

    let apply_block_result = stf.apply_slot(
        &current_root,
        stf_state,
        Default::default(),
        &setup_block_header,
        &Default::default(),
        setup_blobs.as_iters(),
        ExecutionContext::Node,
    );
    current_root = apply_block_result.state_root;

    let mut data_to_commit = SlotCommit::new(MockBlock {
        header: setup_block_header.clone(),
        batch_blobs: vec![setup_blob],
        ..Default::default()
    });
    data_to_commit.add_batch(apply_block_result.batch_receipts[0].clone());
    let mut ledger_change_set = ledger_db
        .materialize_slot(data_to_commit, current_root.as_ref())
        .unwrap();
    let finalized_slot_changes = ledger_db.materialize_latest_finalize_slot(1).unwrap();
    ledger_change_set.merge(finalized_slot_changes);

    storage_manager
        .save_change_set(
            &setup_block_header,
            apply_block_result.change_set,
            ledger_change_set,
        )
        .unwrap();
    storage_manager.finalize(&setup_block_header).unwrap();
    ledger_db.send_notifications();

    // Blocks for benchmark
    let mut blocks = Vec::with_capacity(params.blocks as usize);

    for (idx, (_, pk)) in sender_keys.iter().enumerate() {
        let mut txs = vec![];
        for tx_idx in 0..params.transactions_per_block {
            let nonce = if idx == 0 {
                deployer_nonce + tx_idx
            } else {
                tx_idx
            };
            let receiver_address: <TestSpec as Spec>::Address =
                (&TestPrivateKey::generate().pub_key()).into();
            let msg: sov_bank::CallMessage<TestSpec> =
                sov_bank::CallMessage::<TestSpec>::Transfer {
                    to: receiver_address,
                    coins: Coins {
                        amount: 1,
                        token_id,
                    },
                };
            let ser_tx = signed_bank_tx(msg, pk, nonce);
            txs.push(ser_tx);
        }

        let batch = Batch::new(txs);
        let blob =
            MockBlob::new_with_hash(borsh::to_vec(&batch).unwrap(), MockAddress::new([0; 32]));

        blocks.push(MockBlock {
            header: MockBlockHeader::from_height(idx as u64 + 2),
            batch_blobs: vec![blob],
            ..Default::default()
        });
    }

    (storage_manager, current_root, blocks)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let params = BenchParams::new();
    let mut num_success_txns = 0;
    let temp_dir = TempDir::new().expect("Unable to create temporary directory");
    let (mut storage_manager, mut current_root, blocks) = setup(&params, temp_dir.path());

    // 3 blocks to finalization
    let fork_length = 3;
    let blocks_num = blocks.len() as u64;
    let first_block_height = blocks
        .first()
        .expect("There should be at least 1 block")
        .header()
        .height();

    let (_, ledger_storage) = storage_manager.create_bootstrap_state().unwrap();
    let mut ledger_db = LedgerDb::with_reader(ledger_storage).unwrap();
    let stf = BenchStf::new();

    let total = Instant::now();
    let mut apply_block_time = Duration::default();

    for filtered_block in blocks {
        let (stf_state, ledger_storage) = storage_manager
            .create_state_for(filtered_block.header())
            .unwrap();
        ledger_db.replace_reader(ledger_storage);

        let mut data_to_commit = SlotCommit::new(filtered_block.clone());
        let MockBlock {
            header: filtered_header,
            proof_blobs,
            batch_blobs,
            validity_cond,
            ..
        } = filtered_block;

        let mut relevant_blobs = RelevantBlobs::<MockBlob> {
            proof_blobs,
            batch_blobs,
        };

        let now = Instant::now();
        let apply_block_result = stf.apply_slot(
            &current_root,
            stf_state,
            Default::default(),
            &filtered_header,
            &validity_cond,
            relevant_blobs.as_iters(),
            ExecutionContext::Node,
        );
        apply_block_time += now.elapsed();
        current_root = apply_block_result.state_root;

        for receipt in apply_block_result.batch_receipts {
            for t in &receipt.tx_receipts {
                if t.receipt.is_successful() {
                    num_success_txns += 1;
                } else {
                    println!("E: {:?}", t.receipt);
                }
            }
            data_to_commit.add_batch(receipt);
        }

        let mut ledger_change_set = ledger_db
            .materialize_slot(data_to_commit, current_root.as_ref())
            .unwrap();

        let header_to_finalize = match filtered_header.height().checked_sub(fork_length) {
            None => None,
            Some(height_to_finalize) => {
                if height_to_finalize >= first_block_height {
                    let finalized_slot_changes = ledger_db
                        .materialize_latest_finalize_slot(height_to_finalize)
                        .unwrap();
                    ledger_change_set.merge(finalized_slot_changes);
                    Some(MockBlockHeader::from_height(height_to_finalize))
                } else {
                    None
                }
            }
        };
        storage_manager
            .save_change_set(
                &filtered_header,
                apply_block_result.change_set,
                ledger_change_set,
            )
            .unwrap();

        if let Some(header) = header_to_finalize {
            storage_manager.finalize(&header).unwrap();
        }
    }

    let total = total.elapsed();
    assert_eq!(
        blocks_num * params.transactions_per_block,
        num_success_txns,
        "Not enough successful transactions, something is broken"
    );
    if params.timer_output {
        print_times(
            total,
            apply_block_time,
            blocks_num,
            params.transactions_per_block,
            num_success_txns,
        );
    }
    Ok(())
}
