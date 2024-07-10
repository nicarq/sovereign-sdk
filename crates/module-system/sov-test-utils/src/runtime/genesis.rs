use std::marker::PhantomData;

use sov_attester_incentives::{AttesterIncentives, AttesterIncentivesConfig};
use sov_bank::{Bank, BankConfig};
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::{CryptoSpec, DaSpec, Genesis, PrivateKey, Spec};
use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};

use crate::TestSpec;

// Constants used in the genesis configuration of the test runtime
const DEFAULT_MIN_USER_BOND: u64 = 100_000_000;
const DEFAULT_MAX_ATTESTED_HEIGHT: u64 = 0;
const DEFAULT_LIGHT_CLIENT_FINALIZED_HEIGHT: u64 = 0;
const DEFAULT_ROLLUP_FINALITY_PERIOD: u64 = 1;
const DEFAULT_GAS_TOKEN_NAME: &str = "TestGasToken";
const DEFAULT_BONDED_BALANCE: u64 = 100_000_000;
const DEFAULT_ADDITIONAL_BALANCE: u64 = 1_000_000_000;

/// A genesis config for a minimal optimsitic runtime
pub struct MinimalOptimisticGenesisConfig<S: Spec, Da: DaSpec> {
    pub sequencer_registry: <SequencerRegistry<S, Da> as Genesis>::Config,
    pub attester_incentives: <AttesterIncentives<S, Da> as Genesis>::Config,
    pub bank: <Bank<S> as Genesis>::Config,
}

/// A representation of a simple user that is not staked at genesis.
#[derive(Debug, Clone)]
pub struct User<S: Spec> {
    private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    balance: u64,
}

impl<S: Spec> User<S> {
    /// Creates a new user with the given private key and balance.
    pub fn new(private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey, balance: u64) -> Self {
        Self {
            private_key,
            balance,
        }
    }

    pub fn generate(balance: u64) -> Self {
        Self {
            private_key: <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate(),
            balance,
        }
    }

    /// Returns the address of the user.
    pub fn address(&self) -> <S as Spec>::Address {
        <S as Spec>::Address::from(&self.private_key.pub_key())
    }

    /// Returns the private key of the user.
    pub fn private_key(&self) -> &<S::CryptoSpec as CryptoSpec>::PrivateKey {
        &self.private_key
    }

    /// Returns the balance of the user.
    pub fn balance(&self) -> u64 {
        self.balance
    }
}

pub trait StakedUser<S: Spec>: Into<User<S>> {
    /// Returns the private key of the staked user.
    fn private_key(&self) -> &<S::CryptoSpec as CryptoSpec>::PrivateKey;

    /// Only returns the bank balance of the staked user. Ie, the balance that is not staked.
    fn free_balance(&self) -> u64;

    /// Returns the bond amount of the staked user.
    fn bond(&self) -> u64;

    /// The total balance of the staked user, including the bond and any additional balance.
    fn total_balance(&self) -> u64 {
        self.bond() + self.free_balance()
    }

    /// Compute and return the address of the staked user.
    fn address(&self) -> S::Address {
        <S as Spec>::Address::from(&self.private_key().pub_key())
    }
}

impl<S: Spec> From<SimpleStakedUser<S>> for User<S> {
    fn from(staked_user: SimpleStakedUser<S>) -> Self {
        Self {
            private_key: staked_user.private_key,
            balance: staked_user.additional_balance.unwrap_or_default(),
        }
    }
}

impl<S: Spec, Da: DaSpec> From<Sequencer<S, Da>> for User<S> {
    fn from(sequencer: Sequencer<S, Da>) -> Self {
        Self {
            private_key: sequencer.private_key,
            balance: sequencer.additional_balance.unwrap_or_default(),
        }
    }
}

/// A simple representation of a user that is staked at genesis.
#[derive(Debug, Clone)]
pub struct SimpleStakedUser<S: Spec> {
    private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    /// The amount of tokens to bond at genesis. These tokens will be minted by the bank.
    bond: u64,
    /// Any additional (not bonded) balance that the bank should mint for the user.
    additional_balance: Option<u64>,
}

impl<S: Spec> StakedUser<S> for SimpleStakedUser<S> {
    /// Returns the private key of the staked user.
    fn private_key(&self) -> &<S::CryptoSpec as CryptoSpec>::PrivateKey {
        &self.private_key
    }

    /// Only returns the bank balance of the staked user. Ie, the balance that is not staked.
    fn free_balance(&self) -> u64 {
        self.additional_balance.unwrap_or(0)
    }

    /// Returns the bond amount of the staked user.
    fn bond(&self) -> u64 {
        self.bond
    }
}

/// A representation of a sequencer at genesis.
#[derive(Debug, Clone)]
pub struct Sequencer<S: Spec, Da: DaSpec> {
    private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    da_address: Da::Address,
    /// The amount of tokens to bond at genesis. These tokens will be minted by the bank.
    bond: u64,
    /// Any additional (not bonded) balance that the bank should mint for the attester.
    additional_balance: Option<u64>,
}

impl<S: Spec, Da: DaSpec> StakedUser<S> for Sequencer<S, Da> {
    /// Returns the private key of the sequencer.
    fn private_key(&self) -> &<S::CryptoSpec as CryptoSpec>::PrivateKey {
        &self.private_key
    }

    /// Returns the stake amount of the sequencer.
    fn bond(&self) -> u64 {
        self.bond
    }

    /// Returns the balance of the sequencer.
    fn free_balance(&self) -> u64 {
        self.additional_balance.unwrap_or(0)
    }
}

impl<S: Spec, Da: DaSpec> Sequencer<S, Da> {
    /// Returns the DA address of the sequencer.
    pub fn da_address(&self) -> &Da::Address {
        &self.da_address
    }
}

/// A convenient high-level representation of an optimistic genesis config. This config
/// is expressed in terms of abstract entities like Attesters and Sequencers, rather than
/// the low level details of accounts with balances held by several different modules.
///
/// This type can be converted into a low-level [`MinimalOptimisticGenesisConfig`] using
/// the [`From`] trait.
#[derive(Debug, Clone)]
pub struct HighLevelOptimisticGenesisConfig<S: Spec, Da: DaSpec> {
    pub initial_attester: SimpleStakedUser<S>,
    pub initial_challenger: SimpleStakedUser<S>,
    pub initial_sequencer: Sequencer<S, Da>,
    pub additional_accounts: Vec<User<S>>,
    pub gas_token_name: String,
}

impl<S: Spec, Da: DaSpec> HighLevelOptimisticGenesisConfig<S, Da> {
    /// Creates a new high-level genesis config with the given initial attester and sequencer using
    /// the default gas token name.
    pub fn with_defaults(
        initial_attester: SimpleStakedUser<S>,
        initial_challenger: SimpleStakedUser<S>,
        initial_sequencer: Sequencer<S, Da>,
        additional_accounts: Vec<User<S>>,
    ) -> Self {
        Self {
            initial_attester,
            initial_challenger,
            initial_sequencer,
            additional_accounts,
            gas_token_name: DEFAULT_GAS_TOKEN_NAME.to_string(),
        }
    }
}

impl HighLevelOptimisticGenesisConfig<TestSpec, MockDaSpec> {
    /// Generates a new high-level genesis config with random addresses, constant amounts (1_000_000_000 tokens)
    /// and no additional accounts.
    pub fn generate() -> Self {
        Self::generate_with_additional_accounts(0)
    }

    /// Generates a new high-level genesis config with random addresses and constant amounts (1_000_000_000 tokens)
    /// and `num_accounts` additional accounts.
    pub fn generate_with_additional_accounts(num_accounts: usize) -> Self {
        let attester = SimpleStakedUser {
            private_key: <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate(),
            bond: DEFAULT_BONDED_BALANCE,
            additional_balance: Some(DEFAULT_ADDITIONAL_BALANCE), // Give the attester extra tokens to pay for gas
        };
        let challenger = SimpleStakedUser {
            private_key: <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate(),
            bond: DEFAULT_BONDED_BALANCE,
            additional_balance: Some(DEFAULT_ADDITIONAL_BALANCE), // Give the attester extra tokens to pay for gas
        };
        let sequencer = Sequencer {
            private_key: <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate(),
            da_address: MockAddress::from([172; 32]),
            bond: DEFAULT_BONDED_BALANCE,
            additional_balance: Some(DEFAULT_ADDITIONAL_BALANCE),
        };

        let mut additional_accounts = Vec::with_capacity(num_accounts);

        for _ in 0..num_accounts {
            additional_accounts.push(User::<TestSpec>::generate(DEFAULT_ADDITIONAL_BALANCE));
        }

        Self::with_defaults(attester, challenger, sequencer, additional_accounts)
    }
}

impl<S: Spec, Da: DaSpec> From<HighLevelOptimisticGenesisConfig<S, Da>>
    for MinimalOptimisticGenesisConfig<S, Da>
{
    fn from(high_level: HighLevelOptimisticGenesisConfig<S, Da>) -> Self {
        Self::from_args(
            high_level.initial_attester,
            high_level.initial_challenger,
            high_level.initial_sequencer,
            high_level.additional_accounts.as_slice(),
            high_level.gas_token_name,
        )
    }
}

impl<S: Spec, Da: DaSpec> MinimalOptimisticGenesisConfig<S, Da> {
    pub fn from_args(
        initial_attester: SimpleStakedUser<S>,
        initial_challenger: SimpleStakedUser<S>,
        initial_sequencer: Sequencer<S, Da>,
        additional_accounts: &[User<S>],
        gas_token_name: String,
    ) -> Self {
        Self {
            sequencer_registry: SequencerConfig {
                seq_rollup_address: initial_sequencer.address().clone(),
                seq_da_address: initial_sequencer.da_address.clone(),
                minimum_bond: initial_sequencer.bond,
                is_preferred_sequencer: true,
            },
            attester_incentives: AttesterIncentivesConfig {
                minimum_attester_bond: DEFAULT_MIN_USER_BOND,
                minimum_challenger_bond: DEFAULT_MIN_USER_BOND,
                initial_attesters: vec![(
                    initial_attester.address().clone(),
                    initial_attester.bond,
                )],
                rollup_finality_period: DEFAULT_ROLLUP_FINALITY_PERIOD,
                maximum_attested_height: DEFAULT_MAX_ATTESTED_HEIGHT,
                light_client_finalized_height: DEFAULT_LIGHT_CLIENT_FINALIZED_HEIGHT,
                phantom_data: PhantomData,
            },

            bank: BankConfig {
                gas_token_config: sov_bank::GasTokenConfig {
                    token_name: gas_token_name,
                    address_and_balances: {
                        let mut additional_accounts_vec = additional_accounts.to_vec();
                        additional_accounts_vec.append(&mut vec![
                            initial_sequencer.into(),
                            initial_attester.into(),
                            initial_challenger.into(),
                        ]);
                        additional_accounts_vec
                            .into_iter()
                            .map(|user| (user.address(), user.balance()))
                            .collect()
                    },
                    authorized_minters: vec![],
                },
                tokens: vec![],
            },
        }
    }
}
