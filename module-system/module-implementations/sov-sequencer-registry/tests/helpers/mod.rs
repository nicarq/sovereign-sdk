use jsonrpsee::core::RpcResult;
use sov_bank::TokenId;
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::digest::Digest;
use sov_modules_api::{Address, CryptoSpec, DaSpec, Module, Spec, StateAccessor, WorkingSet};
use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};

type S = sov_test_utils::TestSpec;
pub type Da = MockDaSpec;

pub const GENESIS_SEQUENCER_KEY: &str = "sequencer_1";
pub const GENESIS_SEQUENCER_DA_ADDRESS: [u8; 32] = [1; 32];
pub const ANOTHER_SEQUENCER_KEY: &str = "sequencer_2";
#[allow(dead_code)]
pub const ANOTHER_SEQUENCER_DA_ADDRESS: [u8; 32] = [2; 32];
pub const UNKNOWN_SEQUENCER_KEY: &str = "sequencer_3";
#[allow(dead_code)]
pub const REWARD_SEQUENCER_KEY: &str = "sequencer_4";
#[allow(dead_code)]
pub const UNKNOWN_SEQUENCER_DA_ADDRESS: [u8; 32] = [3; 32];
pub const LOW_FUND_KEY: &str = "zero_funds";
pub const INITIAL_BALANCE: u64 = 210;
#[allow(dead_code)]
pub const INITIAL_BALANCE_LARGE: u64 = 2100;
pub const LOCKED_AMOUNT: u64 = 200;
#[allow(dead_code)]
pub const GENESIS_TOKEN_NAME: &str = "initial_token";

pub struct TestSequencer {
    pub bank: sov_bank::Bank<S>,
    pub bank_config: sov_bank::BankConfig<S>,

    pub registry: SequencerRegistry<S, Da>,
    pub sequencer_config: SequencerConfig<S, Da>,
}

impl TestSequencer {
    pub fn genesis(&self, working_set: &mut WorkingSet<S>) {
        self.bank.genesis(&self.bank_config, working_set).unwrap();

        self.registry
            .genesis(&self.sequencer_config, working_set)
            .unwrap();
    }

    #[allow(dead_code)]
    pub fn query_balance_via_bank(
        &self,
        working_set: &mut impl StateAccessor,
    ) -> RpcResult<sov_bank::BalanceResponse> {
        let amount = self.bank.get_balance_of(
            &self.sequencer_config.seq_rollup_address,
            self.sequencer_config.coins_to_lock.token_id,
            working_set,
        );
        Ok(sov_bank::BalanceResponse { amount })
    }

    #[allow(dead_code)]
    pub fn query_balance(
        &self,
        user_address: <S as Spec>::Address,
        working_set: &mut WorkingSet<S>,
    ) -> RpcResult<sov_bank::BalanceResponse> {
        self.bank.balance_of(
            None,
            user_address,
            self.sequencer_config.coins_to_lock.token_id,
            working_set,
        )
    }

    #[allow(dead_code)]
    pub fn query_sender_balance(
        &self,
        user_address: &<Da as DaSpec>::Address,
        working_set: &mut WorkingSet<S>,
    ) -> Option<sov_bank::Amount> {
        self.registry.get_sender_balance(user_address, working_set)
    }

    #[allow(dead_code)]
    pub fn query_if_sequencer_is_allowed(
        &self,
        user_address: &<Da as DaSpec>::Address,
        working_set: &mut impl StateAccessor,
    ) -> bool {
        self.registry.is_sender_allowed(user_address, working_set)
    }

    #[allow(dead_code)]
    pub fn set_coins_amount_to_lock(
        &self,
        amount: sov_bank::Amount,
        working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<()> {
        self.registry.set_coins_amount_to_lock(amount, working_set)
    }
}

pub fn create_bank_config() -> (sov_bank::BankConfig<S>, <S as Spec>::Address) {
    let seq_address = generate_address(GENESIS_SEQUENCER_KEY);

    let gas_token_config = sov_bank::GasTokenConfig {
        token_name: GENESIS_TOKEN_NAME.to_owned(),
        address_and_balances: vec![
            (seq_address, INITIAL_BALANCE),
            (generate_address(ANOTHER_SEQUENCER_KEY), INITIAL_BALANCE),
            (generate_address(UNKNOWN_SEQUENCER_KEY), INITIAL_BALANCE),
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

#[allow(dead_code)]
pub fn create_bank_config_large_balance() -> (sov_bank::BankConfig<S>, <S as Spec>::Address) {
    let seq_address = generate_address(GENESIS_SEQUENCER_KEY);

    let gas_token_config = sov_bank::GasTokenConfig {
        token_name: GENESIS_TOKEN_NAME.to_owned(),
        address_and_balances: vec![
            (seq_address, INITIAL_BALANCE_LARGE),
            (
                generate_address(ANOTHER_SEQUENCER_KEY),
                INITIAL_BALANCE_LARGE,
            ),
            (
                generate_address(UNKNOWN_SEQUENCER_KEY),
                INITIAL_BALANCE_LARGE,
            ),
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
    token_id: TokenId,
) -> SequencerConfig<S, Da> {
    SequencerConfig {
        seq_rollup_address,
        seq_da_address: MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS),
        coins_to_lock: sov_bank::Coins {
            amount: LOCKED_AMOUNT,
            token_id,
        },
        is_preferred_sequencer: false,
    }
}

pub fn create_test_sequencer() -> TestSequencer {
    let bank = sov_bank::Bank::<S>::default();
    let (bank_config, seq_rollup_address) = create_bank_config();

    let token_id = sov_bank::GAS_TOKEN_ID;

    let registry = SequencerRegistry::<S, Da>::default();
    let sequencer_config = create_sequencer_config(seq_rollup_address, token_id);

    TestSequencer {
        bank,
        bank_config,
        registry,
        sequencer_config,
    }
}

#[allow(dead_code)]
pub fn create_test_sequencer_large_balance() -> TestSequencer {
    let bank = sov_bank::Bank::<S>::default();
    let (bank_config, seq_rollup_address) = create_bank_config_large_balance();

    let token_id = sov_bank::GAS_TOKEN_ID;

    let registry = SequencerRegistry::<S, Da>::default();
    let sequencer_config = create_sequencer_config(seq_rollup_address, token_id);

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
