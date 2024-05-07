use sov_bank::{Amount, Payable, GAS_TOKEN_ID};
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::digest::Digest;
use sov_modules_api::{Address, CryptoSpec, DaSpec, Module, Spec, StateAccessor, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;

use crate::{SequencerConfig, SequencerRegistry};

pub type S = sov_test_utils::TestSpec;
pub type Da = MockDaSpec;

pub const GENESIS_SEQUENCER_KEY: &str = "sequencer_1";
pub const GENESIS_SEQUENCER_DA_ADDRESS: [u8; 32] = [1; 32];
pub const ANOTHER_SEQUENCER_KEY: &str = "sequencer_2";

pub const ANOTHER_SEQUENCER_DA_ADDRESS: [u8; 32] = [2; 32];
pub const UNKNOWN_SEQUENCER_KEY: &str = "sequencer_3";

pub const REWARD_SEQUENCER_KEY: &str = "sequencer_4";

pub const UNKNOWN_SEQUENCER_DA_ADDRESS: [u8; 32] = [3; 32];
pub const LOW_FUND_KEY: &str = "zero_funds";
pub const INITIAL_BALANCE: u64 = 210;

pub const INITIAL_BALANCE_LARGE: u64 = 2100;
pub const LOCKED_AMOUNT: u64 = 200;

pub const GENESIS_TOKEN_NAME: &str = "initial_token";

pub struct TestSequencer {
    pub bank: sov_bank::Bank<S>,
    pub bank_config: sov_bank::BankConfig<S>,

    pub registry: SequencerRegistry<S, Da>,
    pub sequencer_config: SequencerConfig<S, Da>,
}

impl TestSequencer {
    /// Simple helper that creates a test sequencer, initializes it with genesis data and verifies that the initialization was successful.
    pub fn initialize_test(
        initial_balance: u64,
        with_preferred_sequencer: bool,
    ) -> (TestSequencer, WorkingSet<S>) {
        let test_sequencer = create_test_sequencer(initial_balance, with_preferred_sequencer);
        let tmpdir = tempfile::tempdir().unwrap();
        let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
        test_sequencer.genesis(&mut working_set);

        // Check that genesis has been performed correctly
        let sequencer_address = generate_address(GENESIS_SEQUENCER_KEY);

        // The genesis sequencer address should be registered
        let registry_response = test_sequencer
            .registry
            .sequencer_address(
                MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS),
                &mut working_set,
            )
            .unwrap();
        assert_eq!(Some(sequencer_address), registry_response.address);

        // The genesis sequencer balance should be the initial balance minus the locked amount
        let balance_after_genesis = test_sequencer
            .query_sequencer_balance(&mut working_set)
            .unwrap();

        assert_eq!(initial_balance - LOCKED_AMOUNT, balance_after_genesis);

        (test_sequencer, working_set)
    }

    pub fn genesis(&self, working_set: &mut WorkingSet<S>) {
        self.bank.genesis(&self.bank_config, working_set).unwrap();

        self.registry
            .genesis(&self.sequencer_config, working_set)
            .unwrap();
    }

    pub fn query_sequencer_balance(&self, working_set: &mut impl StateAccessor) -> Option<Amount> {
        self.bank.get_balance_of(
            &self.sequencer_config.seq_rollup_address,
            GAS_TOKEN_ID,
            working_set,
        )
    }

    pub fn query_balance(
        &self,
        user_address: impl Payable<S>,
        working_set: &mut impl StateAccessor,
    ) -> Option<Amount> {
        self.bank
            .get_balance_of(user_address, GAS_TOKEN_ID, working_set)
    }

    pub fn query_sender_balance(
        &self,
        user_address: &<Da as DaSpec>::Address,
        working_set: &mut WorkingSet<S>,
    ) -> Option<sov_bank::Amount> {
        self.registry.get_sender_balance(user_address, working_set)
    }

    pub fn query_if_sequencer_is_allowed(
        &self,
        user_address: &<Da as DaSpec>::Address,
        working_set: &mut impl StateAccessor,
    ) -> bool {
        self.registry
            .is_sender_allowed(user_address, working_set)
            .is_ok()
    }

    pub fn set_coins_amount_to_lock(
        &self,
        amount: sov_bank::Amount,
        working_set: &mut WorkingSet<S>,
    ) {
        self.registry.minimum_bond.set(&amount, working_set);
    }
}

pub fn create_bank_config(initial_balance: u64) -> (sov_bank::BankConfig<S>, <S as Spec>::Address) {
    let seq_address = generate_address(GENESIS_SEQUENCER_KEY);

    let gas_token_config = sov_bank::GasTokenConfig {
        token_name: GENESIS_TOKEN_NAME.to_owned(),
        address_and_balances: vec![
            (seq_address, initial_balance),
            (generate_address(ANOTHER_SEQUENCER_KEY), initial_balance),
            (generate_address(UNKNOWN_SEQUENCER_KEY), initial_balance),
            (generate_address(LOW_FUND_KEY), 3),
        ],
        authorized_minters: vec![],
    };

    (
        sov_bank::BankConfig {
            gas_token_config,
            tokens: vec![],
        },
        seq_address,
    )
}

pub fn create_sequencer_config(
    seq_rollup_address: <S as Spec>::Address,
    is_preferred_sequencer: bool,
) -> SequencerConfig<S, Da> {
    SequencerConfig {
        seq_rollup_address,
        seq_da_address: MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS),
        minimum_bond: LOCKED_AMOUNT,
        is_preferred_sequencer,
    }
}

pub fn create_test_sequencer(
    initial_balance: u64,
    with_preferred_sequencer: bool,
) -> TestSequencer {
    let bank = sov_bank::Bank::<S>::default();
    let (bank_config, seq_rollup_address) = create_bank_config(initial_balance);

    let registry = SequencerRegistry::<S, Da>::default();
    let sequencer_config = create_sequencer_config(seq_rollup_address, with_preferred_sequencer);

    TestSequencer {
        bank,
        bank_config,
        registry,
        sequencer_config,
    }
}

pub fn generate_address(key: &str) -> <S as Spec>::Address {
    let hash: [u8; 32] =
        <<S as Spec>::CryptoSpec as CryptoSpec>::Hasher::digest(key.as_bytes()).into();
    Address::from(hash)
}
