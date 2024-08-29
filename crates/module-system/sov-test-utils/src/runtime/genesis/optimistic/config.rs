use std::collections::HashSet;

use sov_accounts::{AccountConfig, AccountData, Accounts};
use sov_attester_incentives::{AttesterIncentives, AttesterIncentivesConfig};
use sov_bank::{Bank, BankConfig, TokenConfig};
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::{DaSpec, Genesis, Spec};
use sov_nonces::Nonces;
use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};

use crate::interface::AsUser;
use crate::runtime::genesis::TestTokenName;
use crate::{
    TestAttester, TestChallenger, TestSequencer, TestSpec, TestUser, UserTokenInfo,
    TEST_DEFAULT_USER_BALANCE, TEST_DEFAULT_USER_STAKE, TEST_GAS_TOKEN_NAME,
    TEST_LIGHT_CLIENT_FINALIZED_HEIGHT, TEST_MAX_ATTESTED_HEIGHT, TEST_ROLLUP_FINALITY_PERIOD,
};

/// A genesis config for a minimal optimsitic runtime
pub struct MinimalOptimisticGenesisConfig<S: Spec, Da: DaSpec> {
    /// The sequencer registry config.
    pub sequencer_registry: <SequencerRegistry<S, Da> as Genesis>::Config,
    /// The attester incentives config.
    pub attester_incentives: <AttesterIncentives<S, Da> as Genesis>::Config,
    /// The bank config.
    pub bank: <Bank<S> as Genesis>::Config,
    /// The accounts config.
    pub accounts: <Accounts<S> as Genesis>::Config,
    /// The nonces config.
    pub nonces: <Nonces<S> as Genesis>::Config,
}

/// A convenient high-level representation of an optimistic genesis config. This config
/// is expressed in terms of abstract entities like Attesters and Sequencers, rather than
/// the low level details of accounts with balances held by several different modules.
///
/// This type can be converted into a low-level [`MinimalOptimisticGenesisConfig`] using
/// the [`From`] trait.
#[derive(Debug, Clone)]
pub struct HighLevelOptimisticGenesisConfig<S: Spec, Da: DaSpec> {
    /// The initial attester.
    pub initial_attester: TestAttester<S>,
    /// The initial challenger.
    pub initial_challenger: TestChallenger<S>,
    /// The initial sequencer.
    pub initial_sequencer: TestSequencer<S, Da>,
    /// Additional accounts to be added to the genesis state.
    pub additional_accounts: Vec<TestUser<S>>,
    /// The name of the gas token.
    pub gas_token_name: String,
}

impl<S: Spec, Da: DaSpec> HighLevelOptimisticGenesisConfig<S, Da> {
    /// Creates a new high-level genesis config with the given initial attester and sequencer using
    /// the default gas token name.
    pub fn with_defaults(
        initial_attester: TestAttester<S>,
        initial_challenger: TestChallenger<S>,
        initial_sequencer: TestSequencer<S, Da>,
        additional_accounts: Vec<TestUser<S>>,
    ) -> Self {
        Self {
            initial_attester,
            initial_challenger,
            initial_sequencer,
            additional_accounts,
            gas_token_name: TEST_GAS_TOKEN_NAME.to_string(),
        }
    }

    /// Returns the list of accounts that have a balance for the given token. The account vector is cloned.
    pub fn get_accounts_for_token(&self, token_name: &TestTokenName) -> Vec<TestUser<S>> {
        self.additional_accounts
            .clone()
            .into_iter()
            .filter(|user| user.token_balance(token_name).is_some())
            .collect()
    }

    /// Returns the list of token names that are used in the genesis config.
    /// Clones the underlying list of token names.
    pub fn token_names(&self) -> Vec<TestTokenName> {
        let mut token_names = HashSet::<&TestTokenName>::new();

        self.additional_accounts.iter().for_each(|user| {
            user.token_balances.iter().for_each(|token_info| {
                token_names.insert(&token_info.token_name);
            });
        });

        token_names.into_iter().cloned().collect()
    }
}

impl HighLevelOptimisticGenesisConfig<TestSpec, MockDaSpec> {
    /// Generates a new high-level genesis config with random addresses, constant amounts (1_000_000_000 tokens)
    /// and no additional accounts.
    pub fn generate() -> Self {
        let prover_sequencer =
            TestUser::generate(TEST_DEFAULT_USER_STAKE * 3 + TEST_DEFAULT_USER_BALANCE);

        let attester = TestAttester {
            user_info: prover_sequencer.clone(),
            bond: TEST_DEFAULT_USER_STAKE,
            slot_to_attest: 1,
        };

        let challenger = TestChallenger {
            user_info: prover_sequencer.clone(),
        };

        let sequencer = TestSequencer {
            user_info: prover_sequencer,
            da_address: MockAddress::from([172; 32]),
            bond: TEST_DEFAULT_USER_STAKE,
        };

        Self::with_defaults(attester, challenger, sequencer, vec![])
    }

    /// Generates a new high-level genesis config with random addresses and constant amounts (1_000_000_000 tokens)
    /// and `num_accounts` additional accounts.
    ///
    /// This is a convenience function for [`Self::add_accounts`]
    pub fn add_accounts_with_default_balance(self, num_accounts: usize) -> Self {
        let mut additional_accounts = Vec::with_capacity(num_accounts);

        for _ in 0..num_accounts {
            additional_accounts.push(TestUser::<TestSpec>::generate(TEST_DEFAULT_USER_BALANCE));
        }

        self.add_accounts(additional_accounts)
    }

    /// Adds a token to the genesis config. Generates a token with an (optional) given name and adds it to the list of tokens.
    /// The token is associated with the given number of accounts, each of which has an initial balance. It is also
    /// possible to specify a minter for the token using the `with_minter` parameter.
    ///
    /// This is a convenience function for [`Self::add_accounts`]
    pub fn add_accounts_with_token(
        self,
        token_name: &TestTokenName,
        with_minter: bool,
        num_accounts: usize,
        account_initial_balance: u64,
    ) -> Self {
        let mut additional_accounts = Vec::with_capacity(num_accounts);

        if with_minter {
            additional_accounts.push(
                TestUser::<TestSpec>::generate(TEST_DEFAULT_USER_BALANCE).add_token_info(
                    UserTokenInfo {
                        token_name: token_name.clone(),
                        balance: account_initial_balance,
                        is_minter: true,
                    },
                ),
            );
        }

        for _ in 0..num_accounts {
            additional_accounts.push(
                TestUser::<TestSpec>::generate(TEST_DEFAULT_USER_BALANCE).add_token_info(
                    UserTokenInfo {
                        token_name: token_name.clone(),
                        balance: account_initial_balance,
                        is_minter: false,
                    },
                ),
            );
        }

        self.add_accounts(additional_accounts)
    }

    /// Adds additional accounts to the genesis config.
    pub fn add_accounts(mut self, mut additional_accounts: Vec<TestUser<TestSpec>>) -> Self {
        self.additional_accounts.append(&mut additional_accounts);
        self
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
    /// Helper function that parses the token configs from the list of test users.
    fn parse_token_configs(test_users: &[TestUser<S>]) -> Vec<TokenConfig<S>> {
        let mut token_configs = Vec::<TokenConfig<S>>::new();

        test_users.iter().for_each(|user| {
            let user_address = user.address();

            user.token_balances.iter().for_each(|token_info| {
                let token_name = &token_info.token_name;
                // If there is no entry for that specific token name, we create a new one.
                let token_config = if let Some(config) = token_configs
                    .iter_mut()
                    .find(|config| config.token_name == token_name.to_string())
                {
                    config
                } else {
                    let initial_token_config = TokenConfig {
                        token_name: token_name.to_string(),
                        token_id: token_name.id(),
                        address_and_balances: Vec::new(),
                        authorized_minters: Vec::new(),
                    };

                    token_configs.push(initial_token_config);

                    token_configs.last_mut().unwrap()
                };

                if token_info.is_minter {
                    token_config.authorized_minters.push(user_address.clone());
                }

                token_config
                    .address_and_balances
                    .push((user_address.clone(), token_info.balance));
            });
        });

        token_configs
    }

    /// Creates a new [`MinimalOptimisticGenesisConfig`] from the given arguments.
    pub fn from_args(
        initial_attester: TestAttester<S>,
        initial_challenger: TestChallenger<S>,
        initial_sequencer: TestSequencer<S, Da>,
        additional_accounts: &[TestUser<S>],
        gas_token_name: String,
    ) -> Self {
        Self {
            sequencer_registry: SequencerConfig {
                seq_rollup_address: initial_sequencer.as_user().address().clone(),
                seq_da_address: initial_sequencer.da_address.clone(),
                minimum_bond: initial_sequencer.bond,
                is_preferred_sequencer: true,
            },
            attester_incentives: AttesterIncentivesConfig {
                minimum_attester_bond: TEST_DEFAULT_USER_STAKE,
                minimum_challenger_bond: TEST_DEFAULT_USER_STAKE,
                initial_attesters: vec![(
                    initial_attester.as_user().address().clone(),
                    initial_attester.bond,
                )],
                rollup_finality_period: TEST_ROLLUP_FINALITY_PERIOD,
                maximum_attested_height: TEST_MAX_ATTESTED_HEIGHT,
                light_client_finalized_height: TEST_LIGHT_CLIENT_FINALIZED_HEIGHT,
            },

            bank: BankConfig {
                gas_token_config: sov_bank::GasTokenConfig {
                    token_name: gas_token_name,
                    address_and_balances: {
                        let mut additional_accounts_vec: Vec<_> = additional_accounts
                            .iter()
                            .map(|user| (user.address(), user.balance()))
                            .collect();

                        let sequencer = initial_sequencer.as_user();
                        let attester = initial_attester.as_user();

                        if sequencer.address() == attester.address() {
                            assert_eq!(sequencer.available_gas_balance, attester.available_gas_balance, "Sequencer and prover balances should be equal if they are the same user");

                            additional_accounts_vec.append(&mut vec![(
                                sequencer.address(),
                                initial_sequencer.bond
                                    + initial_attester.bond
                                    + sequencer.available_gas_balance,
                            )]);
                        } else {
                            // We need to add the bond to the initial balance because genesis deduces the bond from the bank balance.
                            additional_accounts_vec.append(&mut vec![
                                (
                                    initial_sequencer.as_user().address(),
                                    initial_sequencer.bond
                                        + initial_sequencer.as_user().available_gas_balance,
                                ),
                                (
                                    initial_attester.as_user().address(),
                                    initial_attester.bond
                                        + initial_attester.as_user().available_gas_balance,
                                ),
                                (
                                    initial_challenger.as_user().address(),
                                    initial_challenger.as_user().available_gas_balance,
                                ),
                            ]);
                        }

                        additional_accounts_vec
                    },
                    authorized_minters: vec![],
                },
                tokens: Self::parse_token_configs(additional_accounts),
            },
            accounts: AccountConfig {
                accounts: {
                    additional_accounts
                        .iter()
                        .filter_map(|user| {
                            user.custom_credential_id.map(|credential_id| AccountData {
                                credential_id,
                                address: user.address(),
                            })
                        })
                        .collect()
                },
            },
            nonces: (),
        }
    }
}
