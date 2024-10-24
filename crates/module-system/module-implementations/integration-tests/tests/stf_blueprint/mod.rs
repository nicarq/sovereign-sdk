mod registered;

use sov_attester_incentives::AttesterIncentives;
use sov_bank::{Bank, IntoPayable};
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{ApiStateAccessor, DaSpec, EncodeCall, ModuleInfo, PrivateKey, RawTx, Spec};
use sov_sequencer_registry::SequencerRegistry;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{config_gas_token_id, Payable, TestRunner};
use sov_test_utils::{
    generate_optimistic_runtime, TestSequencer, TestUser, TransactionType,
    TEST_DEFAULT_USER_BALANCE,
};
use sov_value_setter::{CallMessage, ValueSetter};

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
) -> u64 {
    let sequencer_module = SequencerRegistry::<S>::default();
    sequencer_module
        .is_sender_allowed(sequencer_da_address, state)
        .unwrap()
        .balance
}

fn setup() -> (TestRunner<IntegTestRuntime<S>, S>, Actors) {
    let mut genesis_config = HighLevelOptimisticGenesisConfig::generate();
    genesis_config
        .additional_accounts
        .push(TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE));

    genesis_config
        .additional_accounts
        .push(TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE));

    let admin_account = genesis_config.additional_accounts[0].clone();
    let additional_account = genesis_config.additional_accounts[1].clone();

    let genesis = GenesisConfig::from_minimal_config(
        genesis_config.clone().into(),
        sov_value_setter::ValueSetterConfig {
            admin: admin_account.address(),
        },
    );

    let runner: TestRunner<IntegTestRuntime<S>, S> =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), Default::default());

    let admin_account = genesis_config.additional_accounts[0].clone();
    let sequencer_account = genesis_config.initial_sequencer.clone();

    (
        runner,
        Actors {
            admin_account,
            not_admin_account: additional_account,
            sequencer_account,
        },
    )
}

fn create_tx_bad_chain_id(
    nonce: u64,
    max_priority_fee_bips: PriorityFeeBips,
    signer: &TestUser<S>,
) -> TransactionType<ValueSetter<S>, S> {
    let encoded_message =
        <IntegTestRuntime<S> as EncodeCall<ValueSetter<S>>>::encode_call(CallMessage::SetValue(8));

    let utx = UnsignedTransaction::new(
        encoded_message.clone(),
        config_value!("CHAIN_ID") + 1,
        max_priority_fee_bips,
        200_000,
        nonce,
        None,
    );

    TransactionType::<ValueSetter<S>, S>::pre_signed(utx, signer.private_key())
}

fn create_tx_bad_sig(
    nonce: u64,
    max_priority_fee_bips: PriorityFeeBips,
    signer: &TestUser<S>,
) -> TransactionType<ValueSetter<S>, S> {
    let encoded_message =
        <IntegTestRuntime<S> as EncodeCall<ValueSetter<S>>>::encode_call(CallMessage::SetValue(8));

    let utx = UnsignedTransaction::<S>::new(
        encoded_message.clone(),
        config_value!("CHAIN_ID"),
        max_priority_fee_bips,
        200_000,
        nonce,
        None,
    );

    let mut signed_tx = Transaction::new_signed_tx(&signer.private_key, utx);

    // Create a signature for a different message so it won't verify in the stf.
    let bad_signature = signer.private_key.sign(&[1, 2, 3]);
    signed_tx.signature = bad_signature;
    let tx = borsh::to_vec(&signed_tx).unwrap();

    TransactionType::PreSigned(RawTx { data: tx })
}

fn create_tx_valid(
    nonce: u64,
    max_priority_fee_bips: PriorityFeeBips,
    signer: &TestUser<S>,
) -> TransactionType<ValueSetter<S>, S> {
    let encoded_message =
        <IntegTestRuntime<S> as EncodeCall<ValueSetter<S>>>::encode_call(CallMessage::SetValue(8));

    let utx = UnsignedTransaction::new(
        encoded_message.clone(),
        config_value!("CHAIN_ID"),
        max_priority_fee_bips,
        200_000,
        nonce,
        None,
    );

    TransactionType::<ValueSetter<S>, S>::pre_signed(utx, signer.private_key())
}

#[derive(PartialEq, Eq)]
enum TxStatus {
    Success,
    BadNonce,
    BadChainId,
    BadSignature,
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

fn create_txs(
    statuses: &[TxStatus],
    max_priority_fee_bips: PriorityFeeBips,
    admin: &TestUser<S>,
    not_admin: &TestUser<S>,
) -> Vec<TransactionType<ValueSetter<S>, S>> {
    let mut nonce = 0;
    let mut reverted_tx_nonce = 0;
    let mut txs = Vec::new();
    for status in statuses {
        match status {
            TxStatus::Success => {
                let tx = create_tx_valid(nonce, max_priority_fee_bips, admin);
                txs.push(tx);
                nonce += 1;
            }
            TxStatus::Reverted => {
                // A call message send by not admin will be reverted.
                let tx = create_tx_valid(reverted_tx_nonce, max_priority_fee_bips, not_admin);
                txs.push(tx);
                reverted_tx_nonce += 1;
            }
            TxStatus::BadNonce => {
                let tx = create_tx_valid(9999, max_priority_fee_bips, admin);
                txs.push(tx);
            }
            TxStatus::BadChainId => {
                let tx = create_tx_bad_chain_id(nonce, max_priority_fee_bips, admin);
                txs.push(tx);
            }
            TxStatus::BadSignature => {
                let tx = create_tx_bad_sig(nonce, max_priority_fee_bips, admin);
                txs.push(tx);
            }
        }
    }
    txs
}

struct Actors {
    admin_account: TestUser<S>,
    not_admin_account: TestUser<S>,
    sequencer_account: TestSequencer<S>,
}

impl Actors {
    fn balances(&self, state: &mut ApiStateAccessor<S>) -> Balances {
        let attester_module = AttesterIncentives::<S>::default();
        Balances {
            admin_balance: get_balance(&self.admin_account.address(), state),
            not_admin_balance: get_balance(&self.not_admin_account.address(), state),
            attester_module_balance: get_balance(attester_module.id().to_payable(), state),
            sequencer_bond: get_seq_bond(&self.sequencer_account.da_address, state),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Balances {
    admin_balance: u64,
    not_admin_balance: u64,
    attester_module_balance: u64,
    sequencer_bond: u64,
}

impl Balances {
    fn total_balance(&self) -> u64 {
        self.admin_balance
            + self.not_admin_balance
            + self.sequencer_bond
            + self.attester_module_balance
    }
}
