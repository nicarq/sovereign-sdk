use std::collections::HashSet;

use sov_accounts::{AccountConfig, AccountData, Accounts};
use sov_attester_incentives::{AttesterIncentives, AttesterIncentivesConfig};
use sov_bank::{Bank, BankConfig, TokenConfig};
use sov_mock_da::MockAddress;
use sov_modules_api::{Gas, GasArray, GasSpec, Genesis, Spec, Zkvm};
use sov_nonces::Nonces;
use sov_prover_incentives::{ProverIncentives, ProverIncentivesConfig};
use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};

use crate::interface::AsUser;
use crate::runtime::genesis::TestTokenName;
use crate::runtime::{BlobStorage, ChainState, ChainStateConfig};
use crate::{
    TestAttester, TestChallenger, TestSequencer, TestSpec, TestUser, UserTokenInfo,
    TEST_DEFAULT_USER_BALANCE, TEST_DEFAULT_USER_STAKE, TEST_GAS_TOKEN_NAME,
    TEST_LIGHT_CLIENT_FINALIZED_HEIGHT, TEST_MAX_ATTESTED_HEIGHT, TEST_ROLLUP_FINALITY_PERIOD,
};

/// A genesis config for a minimal optimsitic runtime
pub struct MinimalOptimisticGenesisConfig<S: Spec> {
    /// The sequencer registry config.
    pub sequencer_registry: <SequencerRegistry<S> as Genesis>::Config,
    /// The attester incentives config.
    pub attester_incentives: <AttesterIncentives<S> as Genesis>::Config,
    /// The prover incentives config.
    pub prover_incentives: <ProverIncentives<S> as Genesis>::Config,
    /// The bank config.
    pub bank: <Bank<S> as Genesis>::Config,
    /// The accounts config.
    pub accounts: <Accounts<S> as Genesis>::Config,
    /// The nonces config.
    pub nonces: <Nonces<S> as Genesis>::Config,
    /// The chain state config.
    pub chain_state: <ChainState<S> as Genesis>::Config,
    /// The blob storage config.
    pub blob_storage: <BlobStorage<S> as Genesis>::Config,
}

/// A convenient high-level representation of an optimistic genesis config. This config
/// is expressed in terms of abstract entities like Attesters and Sequencers, rather than
/// the low level details of accounts with balances held by several different modules.
///
/// This type can be converted into a low-level [`MinimalOptimisticGenesisConfig`] using
/// the [`From`] trait.
#[derive(Debug, Clone)]
pub struct HighLevelOptimisticGenesisConfig<S: Spec> {
    /// The initial attester.
    pub initial_attester: TestAttester<S>,
    /// The initial challenger.
    pub initial_challenger: TestChallenger<S>,
    /// The initial sequencer.
    pub initial_sequencer: TestSequencer<S>,
    /// Additional accounts to be added to the genesis state.
    pub additional_accounts: Vec<TestUser<S>>,
    /// The name of the gas token.
    pub gas_token_name: String,
    /// The inner code commitment.
    pub inner_code_commitment: <S::InnerZkvm as Zkvm>::CodeCommitment,
    /// The outer code commitment.
    pub outer_code_commitment: <S::OuterZkvm as Zkvm>::CodeCommitment,
}

impl<S: Spec> HighLevelOptimisticGenesisConfig<S> {
    /// Creates a new high-level genesis config with the given initial attester and sequencer using
    /// the default gas token name.
    pub fn with_defaults(
        initial_attester: TestAttester<S>,
        initial_challenger: TestChallenger<S>,
        initial_sequencer: TestSequencer<S>,
        additional_accounts: Vec<TestUser<S>>,
        inner_code_commitment: <S::InnerZkvm as Zkvm>::CodeCommitment,
        outer_code_commitment: <S::OuterZkvm as Zkvm>::CodeCommitment,
    ) -> Self {
        Self {
            initial_attester,
            initial_challenger,
            initial_sequencer,
            additional_accounts,
            gas_token_name: TEST_GAS_TOKEN_NAME.to_string(),
            inner_code_commitment,
            outer_code_commitment,
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

impl HighLevelOptimisticGenesisConfig<TestSpec> {
    /// The sequencer address used by [`HighLevelOptimisticGenesisConfig::generate`].
    pub const SEQUENCER_DA_ADDR: MockAddress = MockAddress::new([172; 32]);
    /// Generates a new high-level genesis config with random addresses, constant amounts (1_000_000_000 tokens)
    /// and no additional accounts.
    pub fn generate() -> Self {
        // The stake value is doubled to ensure that sequencers can still send batches when gas price fluctuates
        let user_stake_value = <TestSpec as Spec>::Gas::from(TEST_DEFAULT_USER_STAKE)
            .value(&TestSpec::initial_base_fee_per_gas())
            * 2;

        let prover_sequencer = TestUser::generate(user_stake_value * 3 + TEST_DEFAULT_USER_BALANCE);

        let attester = TestAttester {
            user_info: prover_sequencer.clone(),
            bond: user_stake_value,
            slot_to_attest: 1,
        };

        let challenger = TestChallenger {
            user_info: prover_sequencer.clone(),
        };

        let sequencer = TestSequencer {
            user_info: prover_sequencer,
            da_address: Self::SEQUENCER_DA_ADDR,
            bond: user_stake_value,
        };

        let inner_code_commitment = Default::default();
        let outer_code_commitment = Default::default();

        Self::with_defaults(
            attester,
            challenger,
            sequencer,
            vec![],
            inner_code_commitment,
            outer_code_commitment,
        )
    }

    /// Generates a new high-level genesis config with random addresses and constant amounts (1_000_000_000 tokens)
    /// and `num_accounts` additional accounts.
    ///
    /// This is a convenience function for [`Self::add_accounts`]
    pub fn add_accounts_with_default_balance(self, num_accounts: usize) -> Self {
        self.add_accounts_with_balance(num_accounts, TEST_DEFAULT_USER_BALANCE)
    }

    /// Generates a new high-level genesis config with random addresses and constant amounts (1_000_000_000 tokens)
    /// and `num_accounts` additional accounts.
    ///
    /// This is a convenience function for [`Self::add_accounts`]
    pub fn add_accounts_with_balance(mut self, num_accounts: usize, balance: u64) -> Self {
        for _ in 0..num_accounts {
            self.additional_accounts
                .push(TestUser::<TestSpec>::generate(balance));
        }

        self
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

impl<S: Spec> From<HighLevelOptimisticGenesisConfig<S>> for MinimalOptimisticGenesisConfig<S> {
    fn from(high_level: HighLevelOptimisticGenesisConfig<S>) -> Self {
        Self::from_args(
            high_level.initial_attester,
            high_level.initial_challenger,
            high_level.initial_sequencer,
            high_level.additional_accounts.as_slice(),
            high_level.gas_token_name,
            high_level.inner_code_commitment,
            high_level.outer_code_commitment,
        )
    }
}

impl<S: Spec> MinimalOptimisticGenesisConfig<S> {
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
        initial_sequencer: TestSequencer<S>,
        additional_accounts: &[TestUser<S>],
        gas_token_name: String,
        inner_code_commitment: <S::InnerZkvm as Zkvm>::CodeCommitment,
        outer_code_commitment: <S::OuterZkvm as Zkvm>::CodeCommitment,
    ) -> Self {
        let prover_placeholder = TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE);
        Self {
            sequencer_registry: SequencerConfig {
                seq_rollup_address: initial_sequencer.as_user().address().clone(),
                seq_da_address: initial_sequencer.da_address.clone(),
                seq_bond: initial_sequencer.bond,
                is_preferred_sequencer: true,
            },
            attester_incentives: AttesterIncentivesConfig {
                minimum_attester_bond: S::Gas::from(TEST_DEFAULT_USER_STAKE),
                minimum_challenger_bond: S::Gas::from(TEST_DEFAULT_USER_STAKE),
                initial_attesters: vec![(
                    initial_attester.as_user().address().clone(),
                    initial_attester.bond,
                )],
                rollup_finality_period: TEST_ROLLUP_FINALITY_PERIOD,
                maximum_attested_height: TEST_MAX_ATTESTED_HEIGHT,
                light_client_finalized_height: TEST_LIGHT_CLIENT_FINALIZED_HEIGHT,
            },
            // unused in optimistic mode
            prover_incentives: ProverIncentivesConfig {
                minimum_bond: S::Gas::from(TEST_DEFAULT_USER_STAKE),
                proving_penalty: {
                    let mut user_stake = S::Gas::from(TEST_DEFAULT_USER_STAKE);
                    user_stake.scalar_division(2);
                    user_stake
                },
                initial_provers: vec![(
                    prover_placeholder.address().clone(),
                    prover_placeholder.balance(),
                )],
            },
            bank: BankConfig {
                gas_token_config: sov_bank::GasTokenConfig {
                    token_name: gas_token_name,
                    address_and_balances: {
                        let mut additional_accounts_vec: Vec<_> = additional_accounts
                            .iter()
                            .map(|user| (user.address(), user.balance()))
                            .collect();

                        additional_accounts_vec
                            .push((prover_placeholder.address(), prover_placeholder.balance()));

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
            blob_storage: (),
            chain_state: ChainStateConfig {
                current_time: Default::default(),
                genesis_da_height: 0,
                operating_mode: sov_modules_api::OperatingMode::Optimistic,
                inner_code_commitment,
                outer_code_commitment,
            },
        }
    }
}
