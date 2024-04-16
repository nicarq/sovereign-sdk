use std::env;

use async_trait::async_trait;
use borsh::ser::BorshSerialize;
use demo_stf::runtime::Runtime;
use futures::stream::BoxStream;
use sov_bank::{Bank, Coins};
use sov_mock_da::{
    MockAddress, MockBlob, MockBlock, MockBlockHeader, MockHash, MockValidityCond,
    MockValidityCondChecker, MOCK_SEQUENCER_DA_ADDRESS,
};
use sov_modules_api::transaction::{PriorityFeeBips, Transaction};
use sov_modules_api::{Address, CryptoSpec, EncodeCall, GasUnit, PrivateKey, PublicKey, Spec};
use sov_rollup_interface::da::{BlockHeaderTrait, DaSpec, DaVerifier, Time};
use sov_rollup_interface::services::da::{DaService, RelevantBlobs, RelevantProofs, SlotData};
use sov_test_utils::{TestHasher, TestPrivateKey, TestSpec};

const DEFAULT_CHAIN_ID: u64 = 0;
const DEFAULT_MAX_PRIORITY_FEE: PriorityFeeBips = PriorityFeeBips::from_percentage(0);
const DEFAULT_MAX_FEE: u64 = 0;
const DEFAULT_ESTIMATED_GAS_USAGE: Option<GasUnit<2>> = None;

pub fn sender_address_with_pkey<S: Spec>() -> (Address, TestPrivateKey) {
    let pk = TestPrivateKey::generate();
    let addr = pk.to_address::<<S::CryptoSpec as CryptoSpec>::Hasher, _>();
    (addr, pk)
}

#[derive(Clone, Default)]
/// A simple [`DaService`] for a random number generator.
pub struct RngDaService;

impl RngDaService {
    /// Instantiates a new [`RngDaService`].
    pub fn new() -> Self {
        RngDaService
    }
}

/// A simple DaSpec for a random number generator.
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Eq, Debug, Clone, Default)]
pub struct RngDaSpec;

impl DaSpec for RngDaSpec {
    type SlotHash = MockHash;
    type BlockHeader = MockBlockHeader;
    type BlobTransaction = MockBlob;
    type Address = MockAddress;
    type ValidityCondition = MockValidityCond;
    type Checker = MockValidityCondChecker<MockValidityCond>;
    type InclusionMultiProof = [u8; 32];
    type CompletenessProof = ();
    type ChainParams = ();
}

#[async_trait]
impl DaService for RngDaService {
    type Spec = RngDaSpec;
    type Verifier = RngDaVerifier;
    type FilteredBlock = MockBlock;
    type HeaderStream = BoxStream<'static, anyhow::Result<MockBlockHeader>>;
    type TransactionId = ();
    type Error = anyhow::Error;

    async fn get_block_at(&self, height: u64) -> Result<Self::FilteredBlock, Self::Error> {
        let num_bytes = height.to_le_bytes();
        let mut barray = [0u8; 32];
        barray[..num_bytes.len()].copy_from_slice(&num_bytes);

        let block = MockBlock {
            header: MockBlockHeader {
                hash: barray.into(),
                prev_hash: [0u8; 32].into(),
                height,
                time: Time::now(),
            },
            validity_cond: MockValidityCond { is_valid: true },
            batch_blobs: Default::default(),
            proof_blobs: Default::default(),
        };

        Ok(block)
    }

    async fn get_last_finalized_block_header(
        &self,
    ) -> Result<<Self::Spec as DaSpec>::BlockHeader, Self::Error> {
        todo!()
    }

    async fn subscribe_finalized_header(&self) -> Result<Self::HeaderStream, Self::Error> {
        unimplemented!()
    }

    async fn get_head_block_header(
        &self,
    ) -> Result<<Self::Spec as DaSpec>::BlockHeader, Self::Error> {
        unimplemented!()
    }

    fn extract_relevant_blobs(
        &self,
        block: &Self::FilteredBlock,
    ) -> RelevantBlobs<<Self::Spec as DaSpec>::BlobTransaction> {
        let mut num_txns = 10000;
        if let Ok(val) = env::var("TXNS_PER_BLOCK") {
            num_txns = val
                .parse()
                .expect("TXNS_PER_BLOCK var should be a +ve number");
        }

        let data = if block.header().height() == 1 {
            // creating the token
            generate_create_token_payload(0)
        } else {
            // generating the transfer transactions
            generate_transfers(
                num_txns,
                block
                    .header
                    .height()
                    .checked_sub(2)
                    .expect("invalid block height")
                    .saturating_mul(num_txns as u64),
            )
        };

        let address = MockAddress::from(MOCK_SEQUENCER_DA_ADDRESS);
        let blob = MockBlob::new(data, address, [0u8; 32]);

        RelevantBlobs {
            proof_blobs: vec![],
            batch_blobs: vec![blob],
        }
    }

    async fn get_extraction_proof(
        &self,
        _block: &Self::FilteredBlock,
        _blobs: &RelevantBlobs<<Self::Spec as DaSpec>::BlobTransaction>,
    ) -> RelevantProofs<
        <Self::Spec as DaSpec>::InclusionMultiProof,
        <Self::Spec as DaSpec>::CompletenessProof,
    > {
        unimplemented!()
    }

    async fn send_transaction(&self, _blob: &[u8]) -> Result<(), Self::Error> {
        unimplemented!()
    }

    async fn send_aggregated_zk_proof(&self, _proof: &[u8]) -> Result<(), Self::Error> {
        unimplemented!()
    }

    async fn get_aggregated_proofs_at(&self, _height: u64) -> Result<Vec<Vec<u8>>, Self::Error> {
        unimplemented!()
    }
}

#[derive(Clone)]
pub struct RngDaVerifier;
impl DaVerifier for RngDaVerifier {
    type Spec = RngDaSpec;

    type Error = anyhow::Error;

    fn new(_params: <Self::Spec as DaSpec>::ChainParams) -> Self {
        Self
    }

    fn verify_relevant_tx_list(
        &self,
        _block_header: &<Self::Spec as DaSpec>::BlockHeader,
        _relevant_blobs: &RelevantBlobs<<Self::Spec as DaSpec>::BlobTransaction>,
        _relevant_proofs: RelevantProofs<
            <Self::Spec as DaSpec>::InclusionMultiProof,
            <Self::Spec as DaSpec>::CompletenessProof,
        >,
    ) -> Result<<Self::Spec as DaSpec>::ValidityCondition, Self::Error> {
        Ok(MockValidityCond { is_valid: true })
    }
}

pub fn generate_transfers(n: usize, start_nonce: u64) -> Vec<u8> {
    let token_name = "sov-test-token";
    let (sa, pk) = sender_address_with_pkey::<TestSpec>();
    let token_id = sov_bank::get_token_id::<TestSpec>(token_name, &sa, 11);
    let mut message_vec = vec![];
    for i in 1..n.saturating_add(1) {
        let priv_key = TestPrivateKey::generate();
        let address: <TestSpec as Spec>::Address = priv_key.pub_key().to_address::<TestHasher, _>();
        let msg: sov_bank::CallMessage<TestSpec> = sov_bank::CallMessage::<TestSpec>::Transfer {
            to: address,
            coins: Coins {
                amount: 1,
                token_id,
            },
        };
        let enc_msg =
            <Runtime<TestSpec, RngDaSpec> as EncodeCall<Bank<TestSpec>>>::encode_call(msg);
        let tx = Transaction::<TestSpec>::new_signed_tx(
            &pk,
            enc_msg,
            DEFAULT_CHAIN_ID,
            DEFAULT_MAX_PRIORITY_FEE,
            DEFAULT_MAX_FEE,
            DEFAULT_ESTIMATED_GAS_USAGE,
            start_nonce.wrapping_add(i as u64),
        );
        let ser_tx = tx.try_to_vec().unwrap();
        message_vec.push(ser_tx);
    }
    message_vec.try_to_vec().unwrap()
}

pub fn generate_create_token_payload(start_nonce: u64) -> Vec<u8> {
    let mut message_vec = vec![];

    let (minter_address, pk) = sender_address_with_pkey::<TestSpec>();
    let msg: sov_bank::CallMessage<TestSpec> = sov_bank::CallMessage::<TestSpec>::CreateToken {
        salt: 11,
        token_name: "sov-test-token".to_string(),
        initial_balance: 100000000,
        minter_address,
        authorized_minters: vec![minter_address],
    };
    let enc_msg = <Runtime<TestSpec, RngDaSpec> as EncodeCall<Bank<TestSpec>>>::encode_call(msg);
    let tx = Transaction::<TestSpec>::new_signed_tx(
        &pk,
        enc_msg,
        DEFAULT_CHAIN_ID,
        DEFAULT_MAX_PRIORITY_FEE,
        DEFAULT_MAX_FEE,
        DEFAULT_ESTIMATED_GAS_USAGE,
        start_nonce,
    );
    let ser_tx = tx.try_to_vec().unwrap();
    message_vec.push(ser_tx);
    message_vec.try_to_vec().unwrap()
}
