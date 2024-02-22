use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use serde::Serialize;
use sov_demo_rollup::MockDemoRollup;
use sov_mock_da::{MockAddress, MockBlock, MockDaService};
use sov_modules_api::{CryptoSpec, PrivateKey, Spec};
use sov_rollup_interface::services::da::DaService;
use sov_test_utils::bank_data::BankMessageGenerator;
use sov_test_utils::MessageGenerator;

type S = sov_modules_api::default_spec::DefaultSpec<sov_risc0_adapter::Risc0Verifier>;
type DefaultPrivateKey = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;
type DefaultPublicKey = <<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey;

#[derive(Serialize)]
struct AccountsData {
    pub_keys: Vec<DefaultPublicKey>,
}

const DEFAULT_BLOCKS: u64 = 10;
const DEFAULT_TXNS_PER_BLOCK: u64 = 100;
const DEFAULT_NUM_PUB_KEYS: u64 = 1000;

pub fn generate_genesis_config(config_dir: &str) -> anyhow::Result<()> {
    let num_pub_keys = match env::var("NUM_PUB_KEYS") {
        Ok(num_pub_keys_str) => num_pub_keys_str.parse::<u64>()?,
        Err(_) => {
            println!("NUM_PUB_KEYS not set, using default");
            DEFAULT_NUM_PUB_KEYS
        }
    };

    let file = File::create(Path::join(Path::new(config_dir), "accounts.json")).unwrap();
    let accounts_pub_keys: Vec<_> = (0..num_pub_keys)
        .map(|_| {
            let pkey = DefaultPrivateKey::generate();
            pkey.pub_key()
        })
        .collect();

    let data = AccountsData {
        pub_keys: accounts_pub_keys,
    };

    let data_buf = BufWriter::new(file);
    Ok(serde_json::ser::to_writer(data_buf, &data)?)
}

pub async fn get_bench_blocks() -> anyhow::Result<Vec<MockBlock>> {
    let txns_per_block = match env::var("TXNS_PER_BLOCK") {
        Ok(txns_per_block) => txns_per_block.parse::<u64>()?,
        Err(_) => {
            println!("TXNS_PER_BLOCK not set, using default");
            DEFAULT_TXNS_PER_BLOCK
        }
    };

    let block_cnt = match env::var("BLOCKS") {
        Ok(block_cnt_str) => block_cnt_str.parse::<u64>()?,
        Err(_) => {
            println!("BLOCKS not set, using default");
            DEFAULT_BLOCKS
        }
    };

    let da_service = MockDaService::new(MockAddress::default());
    let mut blocks = vec![];

    let (create_token_message_gen, transfer_message_gen) =
        BankMessageGenerator::generate_token_and_random_transfers(txns_per_block);
    let blob = create_token_message_gen.create_blobs::<<MockDemoRollup as sov_modules_rollup_blueprint::RollupBlueprint>::NativeRuntime>();
    da_service.send_transaction(&blob).await.unwrap();
    let block1 = da_service.get_block_at(1).await.unwrap();
    blocks.push(block1);

    for i in 0..block_cnt {
        let blob = transfer_message_gen.create_blobs::<<MockDemoRollup as sov_modules_rollup_blueprint::RollupBlueprint>::NativeRuntime>();
        da_service.send_transaction(&blob).await.unwrap();
        let blocki = da_service.get_block_at(2 + i).await.unwrap();
        blocks.push(blocki);
    }

    Ok(blocks)
}
