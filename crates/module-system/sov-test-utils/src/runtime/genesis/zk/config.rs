use sov_accounts::{AccountConfig, Accounts};
use sov_attester_incentives::{AttesterIncentives, AttesterIncentivesConfig};
use sov_bank::Bank;
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::{DaSpec, Genesis, Spec};
use sov_nonces::Nonces;
use sov_prover_incentives::ProverIncentives;
use sov_sequencer_registry::SequencerRegistry;

use crate::interface::AsUser;
use crate::runtime::{BankConfig, ProverIncentivesConfig, SequencerConfig};
use crate::{
    TestProver, TestSequencer, TestSpec, TestUser, TEST_DEFAULT_USER_BALANCE,
    TEST_DEFAULT_USER_STAKE, TEST_GAS_TOKEN_NAME,
};

/// Minimal genesis configuration for the zk runtime.
pub struct MinimalZkGenesisConfig<S: Spec, Da: DaSpec> {
    /// The sequencer registry config.
    pub sequencer_registry: <SequencerRegistry<S, Da> as Genesis>::Config,
    /// The prover incentives config.
    pub prover_incentives: <ProverIncentives<S, Da> as Genesis>::Config,
    /// The attester incentives config.
    pub attester_incentives: <AttesterIncentives<S, Da> as Genesis>::Config,
    /// The bank config.
    pub bank: <Bank<S> as Genesis>::Config,
    /// The accounts config.
    pub accounts: <Accounts<S> as Genesis>::Config,
    /// The nonces config.
    pub nonces: <Nonces<S> as Genesis>::Config,
}

/// A convenient high-level representation of a ZK genesis config.
#[derive(Debug, Clone)]
pub struct HighLevelZkGenesisConfig<S: Spec, Da: DaSpec> {
    /// The initial prover.
    pub initial_prover: TestProver<S>,
    /// The initial sequencer.
    pub initial_sequencer: TestSequencer<S, Da>,
    /// Additional accounts to be added to the genesis state.
    pub additional_accounts: Vec<TestUser<S>>,
    /// The name of the gas token
    pub gas_token_name: String,
}

impl<S: Spec, Da: DaSpec> HighLevelZkGenesisConfig<S, Da> {
    /// Creates a new high-level genesis config with the given initial prover and sequencer using
    /// the default gas token name.
    pub fn with_defaults(
        initial_prover: TestProver<S>,
        initial_sequencer: TestSequencer<S, Da>,
        additional_accounts: Vec<TestUser<S>>,
    ) -> Self {
        Self {
            initial_prover,
            initial_sequencer,
            additional_accounts,
            gas_token_name: TEST_GAS_TOKEN_NAME.to_string(),
        }
    }
}

impl HighLevelZkGenesisConfig<TestSpec, MockDaSpec> {
    /// Generates a new high-level genesis config with random addresses, constant amounts (1_000_000_000 tokens)
    /// and no additional accounts.
    pub fn generate() -> Self {
        Self::generate_with_additional_accounts(0)
    }

    /// Generates a new high-level genesis config with random addresses and constant amounts (1_000_000_000 tokens)
    /// and `num_accounts` additional accounts.
    pub fn generate_with_additional_accounts(num_accounts: usize) -> Self {
        // Generate with default stake * 2 because the user will be staked as a sequencer and a
        // prover.
        let prover_sequencer =
            TestUser::generate(TEST_DEFAULT_USER_STAKE * 2 + TEST_DEFAULT_USER_BALANCE);
        let sequencer = TestSequencer {
            user_info: prover_sequencer.clone(),
            da_address: MockAddress::from([172; 32]),
            bond: TEST_DEFAULT_USER_STAKE,
        };
        let prover = TestProver {
            // By default we generate the prover as the same user as the sequencer
            // because provers must be registered sequencers.
            user_info: prover_sequencer,
            bond: TEST_DEFAULT_USER_STAKE,
        };
        let mut additional_accounts = Vec::with_capacity(num_accounts);

        for _ in 0..num_accounts {
            additional_accounts.push(TestUser::<TestSpec>::generate(TEST_DEFAULT_USER_BALANCE));
        }

        Self::with_defaults(prover, sequencer, additional_accounts)
    }
}

impl<S: Spec, Da: DaSpec> From<HighLevelZkGenesisConfig<S, Da>> for MinimalZkGenesisConfig<S, Da> {
    fn from(high_level: HighLevelZkGenesisConfig<S, Da>) -> Self {
        Self::from_args(
            high_level.initial_prover,
            high_level.initial_sequencer,
            high_level.additional_accounts.as_slice(),
            high_level.gas_token_name,
        )
    }
}

impl<S: Spec, Da: DaSpec> MinimalZkGenesisConfig<S, Da> {
    /// Creates a new [`MinimalZkGenesisConfig`] from the given arguments.
    pub fn from_args(
        initial_prover: TestProver<S>,
        initial_sequencer: TestSequencer<S, Da>,
        additional_accounts: &[TestUser<S>],
        gas_token_name: String,
    ) -> Self {
        let attester_placeholder = TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE);
        Self {
            sequencer_registry: SequencerConfig {
                seq_rollup_address: initial_sequencer.as_user().address().clone(),
                seq_da_address: initial_sequencer.da_address.clone(),
                minimum_bond: initial_sequencer.bond,
                is_preferred_sequencer: true,
            },
            prover_incentives: ProverIncentivesConfig {
                minimum_bond: initial_prover.bond,
                proving_penalty: TEST_DEFAULT_USER_STAKE / 2,
                initial_provers: vec![(
                    initial_prover.as_user().address().clone(),
                    initial_prover.bond,
                )],
            },
            // unused in zk mode
            attester_incentives: AttesterIncentivesConfig {
                minimum_attester_bond: TEST_DEFAULT_USER_STAKE,
                minimum_challenger_bond: TEST_DEFAULT_USER_STAKE,
                initial_attesters: vec![(
                    attester_placeholder.address().clone(),
                    attester_placeholder.balance(),
                )],
                rollup_finality_period: 0,
                maximum_attested_height: 0,
                light_client_finalized_height: 0,
            },
            bank: BankConfig {
                gas_token_config: sov_bank::GasTokenConfig {
                    token_name: gas_token_name,
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
                                initial_sequencer.bond
                                    + initial_prover.bond
                                    + sequencer.available_gas_balance,
                            )]);
                        } else {
                            // different users, add separate entries
                            additional_accounts_vec.append(&mut vec![
                                (
                                    initial_sequencer.as_user().address(),
                                    initial_sequencer.bond
                                        + initial_sequencer.as_user().available_gas_balance,
                                ),
                                (
                                    initial_prover.as_user().address(),
                                    initial_prover.bond
                                        + initial_prover.as_user().available_gas_balance,
                                ),
                            ]);
                        }

                        additional_accounts_vec
                    },
                    authorized_minters: vec![],
                },
                tokens: vec![],
            },
            accounts: AccountConfig { accounts: vec![] },
            nonces: (),
        }
    }
}

#[cfg(test)]
mod tests {
    use sov_bank::GAS_TOKEN_ID;
    use sov_mock_da::MockDaSpec;

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

        runner.advance_slots(1).query_state(|state| {
            let bank = crate::runtime::Bank::<S>::default();

            assert_eq!(
                bank.get_balance_of(&sequencer.user_info.address(), GAS_TOKEN_ID, state)
                    .unwrap(),
                Some(sequencer.user_info.balance()),
            );

            let prover_incentives = crate::runtime::ProverIncentives::<S, MockDaSpec>::default();

            assert_eq!(
                prover_incentives
                    .bonded_provers
                    .get(&prover.user_info.address(), state)
                    .unwrap(),
                Some(prover.bond),
                "Should be bonded prover"
            );

            let sequencer_registry = crate::runtime::SequencerRegistry::<S, MockDaSpec>::default();

            assert_eq!(
                sequencer_registry
                    .get_sender_balance(&sequencer.da_address, state)
                    .unwrap(),
                Some(sequencer.bond),
                "Should be bonded sequencer"
            );
        });
    }
}
