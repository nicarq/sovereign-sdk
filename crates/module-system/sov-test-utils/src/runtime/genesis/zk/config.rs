use sov_accounts::{AccountConfig, Accounts};
use sov_attester_incentives::{AttesterIncentives, AttesterIncentivesConfig};
use sov_bank::Bank;
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::{Amount, CodeCommitmentFor, Gas, GasArray, GasSpec, Genesis, Spec};
use sov_prover_incentives::ProverIncentives;
use sov_rollup_interface::common::SlotNumber;
use sov_sequencer_registry::SequencerRegistry;
use sov_uniqueness::Uniqueness;

use crate::interface::AsUser;
use crate::runtime::{
    BankConfig, BlobStorage, ChainState, ChainStateConfig, ProverIncentivesConfig, SequencerConfig,
};
use crate::{
    TestProver, TestSequencer, TestSpec, TestUser, TEST_DEFAULT_USER_BALANCE,
    TEST_DEFAULT_USER_STAKE, TEST_GAS_TOKEN_NAME,
};

/// Minimal genesis configuration for the zk runtime.
pub struct MinimalZkGenesisConfig<S: Spec> {
    /// The sequencer registry config.
    pub sequencer_registry: <SequencerRegistry<S> as Genesis>::Config,
    /// The prover incentives config.
    pub prover_incentives: <ProverIncentives<S> as Genesis>::Config,
    /// The attester incentives config.
    pub attester_incentives: <AttesterIncentives<S> as Genesis>::Config,
    /// The bank config.
    pub bank: <Bank<S> as Genesis>::Config,
    /// The accounts config.
    pub accounts: <Accounts<S> as Genesis>::Config,
    /// The uniqueness config.
    pub uniqueness: <Uniqueness<S> as Genesis>::Config,
    /// The chain state config.
    pub chain_state: <ChainState<S> as Genesis>::Config,
    /// The blob storage config.
    pub blob_storage: <BlobStorage<S> as Genesis>::Config,
}

/// A convenient high-level representation of a ZK genesis config.
#[derive(Debug, Clone)]
pub struct HighLevelZkGenesisConfig<S: Spec> {
    /// The initial prover.
    pub initial_prover: TestProver<S>,
    /// The initial sequencer.
    pub initial_sequencer: TestSequencer<S>,
    /// Additional accounts to be added to the genesis state.
    pub additional_accounts: Vec<TestUser<S>>,
    /// The name of the gas token
    pub gas_token_name: String,
    /// The inner code commitment.
    pub inner_code_commitment: CodeCommitmentFor<S::InnerZkvm>,
    /// The outer code commitment.
    pub outer_code_commitment: CodeCommitmentFor<S::OuterZkvm>,
}

impl<S: Spec> HighLevelZkGenesisConfig<S> {
    /// Creates a new high-level genesis config with the given initial prover and sequencer using
    /// the default gas token name.
    pub fn with_defaults(
        initial_prover: TestProver<S>,
        initial_sequencer: TestSequencer<S>,
        additional_accounts: Vec<TestUser<S>>,
        inner_code_commitment: CodeCommitmentFor<S::InnerZkvm>,
        outer_code_commitment: CodeCommitmentFor<S::OuterZkvm>,
    ) -> Self {
        Self {
            initial_prover,
            initial_sequencer,
            additional_accounts,
            gas_token_name: TEST_GAS_TOKEN_NAME.to_string(),
            inner_code_commitment,
            outer_code_commitment,
        }
    }

    /// Generates a new high-level genesis config with random addresses and constant amounts (1_000_000_000 tokens)
    /// and `num_accounts` additional accounts.
    pub fn generate_with_additional_accounts_and_code_commitments(
        num_accounts: usize,
        inner_code_commitment: CodeCommitmentFor<S::InnerZkvm>,
        outer_code_commitment: CodeCommitmentFor<S::OuterZkvm>,
    ) -> Self
    where
        S: Spec<Da = MockDaSpec>,
    {
        // Generate with default stake * 2 because the user will be staked as a sequencer and a
        // prover.
        let default_user_stake_value =
            <S as Spec>::Gas::from(TEST_DEFAULT_USER_STAKE).value(&S::initial_base_fee_per_gas());

        let prover_sequencer = TestUser::generate(
            default_user_stake_value
                .saturating_mul(Amount::new(2))
                .saturating_add(TEST_DEFAULT_USER_BALANCE),
        );
        let sequencer = TestSequencer {
            user_info: prover_sequencer.clone(),
            da_address: MockAddress::from([172; 32]),
            bond: default_user_stake_value,
        };
        let prover = TestProver {
            // By default we generate the prover as the same user as the sequencer
            // because provers must be registered sequencers.
            user_info: prover_sequencer,
            bond: default_user_stake_value,
        };
        let mut additional_accounts = Vec::with_capacity(num_accounts);

        for _ in 0..num_accounts {
            additional_accounts.push(TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE));
        }

        Self::with_defaults(
            prover,
            sequencer,
            additional_accounts,
            inner_code_commitment,
            outer_code_commitment,
        )
    }

    /// Generates a new high-level genesis config with a given number of additional accounts with random addresses and given balance.
    ///
    pub fn add_accounts_with_balance(mut self, num_accounts: usize, balance: Amount) -> Self {
        for _ in 0..num_accounts {
            self.additional_accounts
                .push(TestUser::<S>::generate(balance));
        }

        self
    }
}

impl HighLevelZkGenesisConfig<TestSpec> {
    /// Generates a new high-level genesis config with random addresses, constant amounts (1_000_000_000 tokens)
    /// and no additional accounts.
    pub fn generate() -> Self {
        Self::generate_with_additional_accounts(0)
    }

    /// Generates a new high-level genesis config with random addresses and constant amounts (1_000_000_000 tokens)
    /// and `num_accounts` additional accounts.
    pub fn generate_with_additional_accounts(num_accounts: usize) -> Self {
        Self::generate_with_additional_accounts_and_code_commitments(
            num_accounts,
            Default::default(),
            Default::default(),
        )
    }
}

impl<S: Spec> From<HighLevelZkGenesisConfig<S>> for MinimalZkGenesisConfig<S> {
    fn from(high_level: HighLevelZkGenesisConfig<S>) -> Self {
        Self::from_args(
            high_level.initial_prover,
            high_level.initial_sequencer,
            high_level.additional_accounts.as_slice(),
            high_level.gas_token_name,
            high_level.inner_code_commitment,
            high_level.outer_code_commitment,
        )
    }
}

impl<S: Spec> MinimalZkGenesisConfig<S> {
    /// Creates a new [`MinimalZkGenesisConfig`] from the given arguments.
    pub fn from_args(
        initial_prover: TestProver<S>,
        initial_sequencer: TestSequencer<S>,
        additional_accounts: &[TestUser<S>],
        gas_token_name: String,
        inner_code_commitment: CodeCommitmentFor<S::InnerZkvm>,
        outer_code_commitment: CodeCommitmentFor<S::OuterZkvm>,
    ) -> Self {
        let attester_placeholder = TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE);
        let default_user_stake = S::Gas::from(TEST_DEFAULT_USER_STAKE);
        Self {
            sequencer_registry: SequencerConfig {
                seq_rollup_address: initial_sequencer.as_user().address().clone(),
                seq_da_address: initial_sequencer.da_address.clone(),
                seq_bond: initial_sequencer.bond,
                is_preferred_sequencer: true,
            },
            prover_incentives: ProverIncentivesConfig {
                minimum_bond: default_user_stake.clone(),
                proving_penalty: {
                    let mut proving_penalty = default_user_stake.clone();
                    proving_penalty.scalar_division(2);
                    proving_penalty
                },
                initial_provers: vec![(
                    initial_prover.as_user().address().clone(),
                    initial_prover.bond,
                )],
            },
            // unused in zk mode
            attester_incentives: AttesterIncentivesConfig {
                minimum_attester_bond: default_user_stake.clone(),
                minimum_challenger_bond: default_user_stake.clone(),
                initial_attesters: vec![(
                    attester_placeholder.address().clone(),
                    attester_placeholder.balance(),
                )],
                rollup_finality_period: SlotNumber::GENESIS,
                maximum_attested_height: SlotNumber::GENESIS,
                light_client_finalized_height: SlotNumber::GENESIS,
            },
            bank: BankConfig {
                gas_token_config: sov_bank::GasTokenConfig {
                    token_name: gas_token_name,
                    token_decimals: None,
                    supply_cap: None,
                    address_and_balances: {
                        let mut additional_accounts_vec: Vec<_> = additional_accounts
                            .iter()
                            .map(|user| (user.address(), user.balance()))
                            .collect();
                        additional_accounts_vec.push((
                            attester_placeholder.address(),
                            attester_placeholder.balance(),
                        ));
                        let sequencer = initial_sequencer.as_user();
                        let prover = initial_prover.as_user();
                        if sequencer.address() == prover.address() {
                            assert_eq!(sequencer.available_gas_balance, prover.available_gas_balance, "Sequencer and prover balances should be equal if they are the same user");
                            // same user, combine the bonds and balances
                            additional_accounts_vec.append(&mut vec![(
                                sequencer.address(),
                                initial_sequencer
                                    .bond
                                    .checked_add(initial_prover.bond)
                                    .unwrap()
                                    .checked_add(sequencer.available_gas_balance)
                                    .unwrap(),
                            )]);
                        } else {
                            // different users, add separate entries
                            additional_accounts_vec.append(&mut vec![
                                (
                                    initial_sequencer.as_user().address(),
                                    initial_sequencer
                                        .bond
                                        .checked_add(
                                            initial_sequencer.as_user().available_gas_balance,
                                        )
                                        .unwrap(),
                                ),
                                (
                                    initial_prover.as_user().address(),
                                    initial_prover
                                        .bond
                                        .checked_add(initial_prover.as_user().available_gas_balance)
                                        .unwrap(),
                                ),
                            ]);
                        }

                        additional_accounts_vec
                    },
                    admins: vec![],
                },
                tokens: vec![],
            },
            accounts: AccountConfig { accounts: vec![] },
            uniqueness: (),
            blob_storage: (),
            chain_state: ChainStateConfig {
                current_time: Default::default(),
                genesis_da_height: 0,
                operating_mode: sov_modules_api::OperatingMode::Zk,
                inner_code_commitment,
                outer_code_commitment,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use sov_bank::config_gas_token_id;

    use super::HighLevelZkGenesisConfig;
    use crate::runtime::TestRunner;
    use crate::{generate_zk_runtime, TestSpec};

    type S = TestSpec;

    generate_zk_runtime!(TestRuntime <= );

    #[test]
    fn test_default_genesis_zk_runtime_config() {
        let genesis_config = HighLevelZkGenesisConfig::generate();
        let sequencer = genesis_config.initial_sequencer.clone();
        let prover = genesis_config.initial_prover.clone();

        assert_eq!(
            sequencer.user_info.address(),
            prover.user_info.address(),
            "Sequencer and prover should be the same user"
        );

        let genesis = GenesisConfig::from_minimal_config(genesis_config.into());
        let mut runner =
            TestRunner::new_with_genesis(genesis.into_genesis_params(), TestRuntime::default());

        runner.advance_slots(1).query_visible_state(|state| {
            let bank = crate::runtime::Bank::<S>::default();

            assert_eq!(
                bank.get_balance_of(&sequencer.user_info.address(), config_gas_token_id(), state)
                    .unwrap(),
                Some(sequencer.user_info.balance()),
            );

            let prover_incentives = crate::runtime::ProverIncentives::<S>::default();

            assert_eq!(
                prover_incentives
                    .bonded_provers
                    .get(&prover.user_info.address(), state)
                    .unwrap(),
                Some(prover.bond),
                "Should be bonded prover"
            );

            let sequencer_registry = crate::runtime::SequencerRegistry::<S>::default();

            assert_eq!(
                sequencer_registry.get_sender_balance_via_api(&sequencer.da_address, state),
                Some(sequencer.bond),
                "Should be bonded sequencer"
            );
        });
    }
}
