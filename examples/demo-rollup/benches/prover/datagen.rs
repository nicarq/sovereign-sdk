use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use demo_stf::genesis_config::{AccountConfig, AccountData};
use sov_address::EthereumAddress;
use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_mock_da::{MockAddress, MockBlock, MockDaService};
use sov_modules_api::{CredentialId, CryptoSpec, PrivateKey, PublicKey, Spec};
use sov_rollup_interface::node::da::DaService;
use sov_test_utils::generators::bank::BankMessageGenerator;
use sov_test_utils::generators::BlobBuildingCtx;
use sov_test_utils::MessageGenerator;

const DEFAULT_BLOCKS: u64 = 10;
const DEFAULT_TXNS_PER_BLOCK: u64 = 100;
const DEFAULT_NUM_PUB_KEYS: u64 = 1000;

type TestPrivateKey<S> = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;
type TestHasher<S> = <<S as Spec>::CryptoSpec as CryptoSpec>::Hasher;

pub fn generate_genesis_config<S: Spec>(config_dir: &str) -> anyhow::Result<()> {
    let num_pub_keys = match env::var("NUM_PUB_KEYS") {
        Ok(num_pub_keys_str) => num_pub_keys_str.parse::<u64>()?,
        Err(_) => {
            println!("NUM_PUB_KEYS not set, using default");
            DEFAULT_NUM_PUB_KEYS
        }
    };

    let mut accs = vec![];

    let priv_keys = (0..num_pub_keys).map(|_| TestPrivateKey::<S>::generate());

    for priv_key in priv_keys {
        let credential_id: CredentialId = priv_key.pub_key().credential_id::<TestHasher<S>>();
        let account = AccountData::<<S as Spec>::Address> {
            credential_id,
            address: <S as Spec>::Address::from(&priv_key.pub_key()),
        };
        accs.push(account);
    }

    let config = AccountConfig::<S> { accounts: accs };

    let file = File::create(Path::join(Path::new(config_dir), "accounts.json")).unwrap();
    let data_buf = BufWriter::new(file);
    Ok(serde_json::ser::to_writer(data_buf, &config)?)
}

const PRIVATE_KEYS_DIR: &str = "../test-data/keys";

fn read_and_parse_private_key<S: Spec>(suffix: &str) -> PrivateKeyAndAddress<S> {
    let data = std::fs::read_to_string(Path::new(PRIVATE_KEYS_DIR).join(suffix))
        .expect("Unable to read file to string");

    let key_and_address: PrivateKeyAndAddress<S> =
        serde_json::from_str(&data).unwrap_or_else(|_| {
            panic!("Unable to convert data {} to PrivateKeyAndAddress", &data);
        });

    assert!(
        key_and_address.is_matching_to_default(),
        "Inconsistent key data"
    );

    key_and_address
}

pub async fn get_bench_blocks<S: Spec>(seq_mode: &BlobBuildingCtx) -> anyhow::Result<Vec<MockBlock>>
where
    <S as Spec>::Address: From<EthereumAddress>,
    <S as Spec>::Address: From<[u8; 32]>,
{
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

    let private_key_and_address: PrivateKeyAndAddress<S> =
        read_and_parse_private_key("minter_private_key.json");

    let (create_token_message_gen, transfer_message_gen) =
        BankMessageGenerator::generate_token_and_random_transfers(
            txns_per_block,
            private_key_and_address.private_key,
        );
    let blob = create_token_message_gen.create_blobs::<demo_stf::runtime::Runtime<S>>(seq_mode);

    let fee = da_service.estimate_fee(blob.len()).await.unwrap();
    da_service
        .send_transaction(&blob, fee)
        .await
        .await
        .unwrap()
        .unwrap();
    let block1 = da_service.get_block_at(1).await.unwrap();
    blocks.push(block1);

    for i in 0..block_cnt {
        let blob = transfer_message_gen.create_blobs::<demo_stf::runtime::Runtime<S>>(seq_mode);
        let fee = da_service.estimate_fee(blob.len()).await.unwrap();
        da_service
            .send_transaction(&blob, fee)
            .await
            .await
            .unwrap()
            .unwrap();
        let blocki = da_service.get_block_at(2 + i).await.unwrap();
        blocks.push(blocki);
    }

    Ok(blocks)
}
