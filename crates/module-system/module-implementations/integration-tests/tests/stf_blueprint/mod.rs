mod registered;
mod unregistered;
use sov_bank::Bank;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{ApiStateAccessor, DaSpec, PrivateKey, Spec};
use sov_sequencer_registry::{AllowedSequencerError, SequencerRegistry};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{config_gas_token_id, Payable, TestRunner};
use sov_test_utils::{
    generate_optimistic_runtime, TestSequencer, TestUser, TEST_DEFAULT_USER_BALANCE,
};
use sov_value_setter::ValueSetter;

type S = sov_test_utils::TestSpec;

generate_optimistic_runtime!(IntegTestRuntime <= value_setter: ValueSetter<S>);

fn get_balance(payable: impl Payable<S>, state: &mut ApiStateAccessor<S>) -> u64 {
    Bank::<S>::default()
        .get_balance_of(payable, config_gas_token_id(), state)
        .unwrap_infallible()
        .unwrap()
}

fn get_seq_bond(
    sequencer_da_address: &<<S as Spec>::Da as DaSpec>::Address,
    state: &mut ApiStateAccessor<S>,
) -> Result<u64, AllowedSequencerError> {
    let sequencer_module = SequencerRegistry::<S>::default();
    sequencer_module
        .is_sender_allowed(sequencer_da_address, state)
        .map(|s| s.balance)
}

#[allow(clippy::type_complexity)]
fn setup(
    nb_of_users: usize,
) -> (
    TestRunner<IntegTestRuntime<S>, S>,
    Vec<TestUser<S>>,
    TestSequencer<S>,
) {
    let mut genesis_config = HighLevelOptimisticGenesisConfig::generate();

    for _ in 0..nb_of_users {
        genesis_config
            .additional_accounts
            .push(TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE));
    }
    let admin = genesis_config.additional_accounts[0].address();

    let genesis = GenesisConfig::from_minimal_config(
        genesis_config.clone().into(),
        sov_value_setter::ValueSetterConfig { admin },
    );

    let runner: TestRunner<IntegTestRuntime<S>, S> =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), Default::default());

    let sequencer_account = genesis_config.initial_sequencer.clone();

    (
        runner,
        genesis_config.additional_accounts,
        sequencer_account,
    )
}

#[derive(PartialEq, Eq)]
enum TxStatus {
    Success,
    BadNonce,
    BadChainId,
    BadSignature,
    SignerDoesNotExist,
    Reverted,
}

impl TxStatus {
    fn nb_of_valid_txs(statuses: &[TxStatus]) -> usize {
        statuses
            .iter()
            .filter(|s| s.is_valid())
            .collect::<Vec<_>>()
            .len()
    }

    fn nb_of_skipped_txs(statuses: &[TxStatus]) -> usize {
        statuses
            .iter()
            .filter(|s| !s.is_valid())
            .collect::<Vec<_>>()
            .len()
    }

    fn is_valid(&self) -> bool {
        // Reverted transactions pass the authentication process; therefore, we count them as valid.
        matches!(self, TxStatus::Success | TxStatus::Reverted)
    }
}

fn create_tx_bad_sig(
    nonce: u64,
    max_priority_fee_bips: PriorityFeeBips,
    signer: &TestUser<S>,
    chain_id: u64,
    encoded_message: Vec<u8>,
) -> Transaction<S> {
    let utx = UnsignedTransaction::<S>::new(
        encoded_message.clone(),
        chain_id,
        max_priority_fee_bips,
        200_000,
        nonce,
        None,
    );

    let mut signed_tx = Transaction::new_signed_tx(&signer.private_key, utx);

    // Create a signature for a different message so it won't verify in the stf.
    let bad_signature = signer.private_key.sign(&[1, 2, 3]);
    signed_tx.signature = bad_signature;

    signed_tx
}

fn create_tx_bad_sender(
    nonce: u64,
    max_priority_fee_bips: PriorityFeeBips,
    chain_id: u64,
    encoded_message: Vec<u8>,
) -> Transaction<S> {
    let utx = UnsignedTransaction::new(
        encoded_message.clone(),
        chain_id,
        max_priority_fee_bips,
        200_000,
        nonce,
        None,
    );

    let signer = TestUser::<S>::generate(0);
    Transaction::<S>::new_signed_tx(signer.private_key(), utx)
}

fn create_tx_valid(
    nonce: u64,
    max_priority_fee_bips: PriorityFeeBips,
    signer: &TestUser<S>,
    chain_id: u64,
    encoded_message: Vec<u8>,
) -> Transaction<S> {
    let utx = UnsignedTransaction::new(
        encoded_message.clone(),
        chain_id,
        max_priority_fee_bips,
        200_000,
        nonce,
        None,
    );

    Transaction::<S>::new_signed_tx(signer.private_key(), utx)
}
