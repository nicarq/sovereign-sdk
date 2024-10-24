use sov_mock_da::MockDaService;
use sov_modules_api::capabilities::TransactionAuthenticator;
use sov_modules_api::digest::Digest;
use sov_modules_api::prelude::*;
use sov_modules_api::transaction::{Transaction, TxDetails, UnsignedTransaction};
use sov_modules_api::{CryptoSpec, FullyBakedTx, RawTx};
use sov_rollup_interface::TxHash;
use sov_sequencer::batch_builders::standard::{StdBatchBuilder, StdBatchBuilderConfig};
use sov_test_utils::generators::bank::BankMessageGenerator;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::sov_paymaster::{
    AuthorizedSequencers, PayeePolicy, PaymasterPolicy, SafeVec,
};
use sov_test_utils::runtime::{AuthenticatorInput, Paymaster, TestOptimisticRuntime};
use sov_test_utils::sequencer::TestSequencerSetup;
use sov_test_utils::{
    EncodeCall, MessageGenerator, TestPrivateKey, TestSpec, TransactionType,
    TEST_DEFAULT_GAS_LIMIT, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE,
};
use sov_value_setter::ValueSetter;

pub type MyBatchBuilder = StdBatchBuilder<(TestSpec, TestOptimisticRuntime<TestSpec>)>;
pub type RT = TestOptimisticRuntime<TestSpec>;

pub async fn new_sequencer() -> TestSequencerSetup<MyBatchBuilder> {
    let dir = tempfile::tempdir().unwrap();
    let da_service = MockDaService::new(HighLevelOptimisticGenesisConfig::SEQUENCER_DA_ADDR);

    let batch_builder_config = StdBatchBuilderConfig {
        mempool_max_txs_count: None,
        max_batch_size_bytes: None,
    };

    TestSequencerSetup::new(dir, da_service, batch_builder_config, vec![], true)
        .await
        .unwrap()
}

pub fn build_tx(
    setup: &TestSequencerSetup<MyBatchBuilder>,
    nonce: u64,
    call_message: Vec<u8>,
) -> RawTx {
    let tx = borsh::to_vec(&Transaction::<TestSpec>::new_signed_tx(
        &setup.admin_private_key,
        UnsignedTransaction::new(
            call_message,
            config_value!("CHAIN_ID"),
            TEST_DEFAULT_MAX_PRIORITY_FEE,
            TEST_DEFAULT_MAX_FEE,
            nonce,
            None,
        ),
    ))
    .unwrap();

    RawTx::new(tx)
}

pub fn wrap_with_auth(raw_tx: RawTx) -> FullyBakedTx {
    TestOptimisticRuntime::<TestSpec>::encode_with_standard_auth(raw_tx)
}

/// Includes transaction data encoded in several ways, for use with different
/// APIs as needed.
#[derive(Debug, Clone)]
pub struct GeneratedTx {
    pub tx_hash: TxHash,
    pub tx_object: Transaction<TestSpec>,
    pub raw_tx: RawTx,
    pub tx_input: AuthenticatorInput,
    pub fully_baked_tx: FullyBakedTx,
}

/// Generates a handful of transactions.
pub fn generate_txs(admin_private_key: TestPrivateKey) -> Vec<GeneratedTx> {
    let bank_generator =
        BankMessageGenerator::<TestSpec>::with_minter_and_transfer(admin_private_key);
    let messages_iter = bank_generator.create_default_messages().into_iter();

    let mut txs = Vec::default();
    for message in messages_iter {
        let tx_object = message.to_tx::<TestOptimisticRuntime<TestSpec>>();
        let raw_tx = RawTx::new(borsh::to_vec(&tx_object).unwrap());

        let tx_hash = TxHash::new(
            <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::Hasher::digest(&raw_tx).into(),
        );

        let tx_input = TestOptimisticRuntime::<TestSpec>::add_standard_auth(raw_tx.clone());
        let fully_baked_tx =
            TestOptimisticRuntime::<TestSpec>::encode_with_standard_auth(raw_tx.clone());

        txs.push(GeneratedTx {
            tx_hash,
            tx_object,
            raw_tx,
            tx_input,
            fully_baked_tx,
        });
    }

    txs
}

/// Generates a paymaster tx signed with the provided key
pub fn generate_paymaster_tx(key: TestPrivateKey) -> RawTx {
    let message = sov_test_utils::runtime::sov_paymaster::CallMessage::RegisterPaymaster {
        policy: PaymasterPolicy {
            default_payee_policy: PayeePolicy::Deny,
            payees: SafeVec::new(),
            authorized_updaters: SafeVec::new(),
            authorized_sequencers: AuthorizedSequencers::All,
        },
    };
    let details = TxDetails::<TestSpec> {
        max_priority_fee_bips: TEST_DEFAULT_MAX_PRIORITY_FEE,
        max_fee: TEST_DEFAULT_MAX_FEE,
        gas_limit: Some(TEST_DEFAULT_GAS_LIMIT.into()),
        chain_id: config_value!("CHAIN_ID"),
    };
    let msg = <RT as EncodeCall<Paymaster<TestSpec>>>::encode_call(message);
    TransactionType::<Paymaster<TestSpec>, TestSpec>::sign(
        msg,
        key,
        details,
        &mut Default::default(),
    )
}

pub fn valid_tx_bytes(
    setup: &TestSequencerSetup<MyBatchBuilder>,
    nonce: u64,
    value_to_set: u32,
) -> RawTx {
    let msg = <TestOptimisticRuntime<TestSpec> as EncodeCall<ValueSetter<TestSpec>>>::encode_call(
        sov_value_setter::CallMessage::SetValue(value_to_set),
    );

    build_tx(setup, nonce, msg)
}
