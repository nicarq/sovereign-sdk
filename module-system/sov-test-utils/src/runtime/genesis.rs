use std::marker::PhantomData;

use sov_attester_incentives::{AttesterIncentives, AttesterIncentivesConfig};
use sov_bank::{Bank, BankConfig};
use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::utils::generate_address;
use sov_modules_api::{DaSpec, Genesis, Spec};
use sov_sequencer_registry::{SequencerConfig, SequencerRegistry};

use crate::TestSpec;

// Constants used in the genesis configuration of the test runtime
const DEFAULT_MIN_USER_BOND: u64 = 100_000;
const DEFAULT_MAX_ATTESTED_HEIGHT: u64 = 0;
const DEFAULT_LIGHT_CLIENT_FINALIZED_HEIGHT: u64 = 0;
const DEFAULT_ROLLUP_FINALITY_PERIOD: u64 = 1;
const DEFAULT_GAS_TOKEN_NAME: &str = "TestGasToken";
const DEFAULT_BONDED_BALANCE: u64 = 100_000;
const DEFAULT_ADDITIONAL_BALANCE: u64 = 1_000_000_000;

/// A genesis config for a minimal optimsitic runtime
pub struct MinimalOptimisticGenesisConfig<S: Spec, Da: DaSpec> {
    pub sequencer_registry: <SequencerRegistry<S, Da> as Genesis>::Config,
    pub attester_incentives: <AttesterIncentives<S, Da> as Genesis>::Config,
    pub bank: <Bank<S> as Genesis>::Config,
}

/// A representation of an Attester at genesis.
pub struct Attester<S: Spec> {
    pub address: S::Address,
    /// The amount of tokens to bond at genesis. These tokens will be minted by the bank.
    pub bond: u64,
    /// Any additional (not bonded) balance that the bank should mint for the attester.
    pub additional_balance: Option<u64>,
}

impl<S: Spec> Attester<S> {
    /// The total balance of the attester, including the bond and any additional balance.
    pub fn total_balance(&self) -> u64 {
        self.bond + self.additional_balance.unwrap_or(0)
    }
}

/// A representation of a sequencer at genesis.
pub struct Sequencer<S: Spec, Da: DaSpec> {
    pub rollup_address: S::Address,
    pub da_address: Da::Address,
    /// The amount of tokens to bond at genesis. These tokens will be minted by the bank.
    pub bond: u64,
    /// Any additional (not bonded) balance that the bank should mint for the attester.
    pub additional_balance: Option<u64>,
}

impl<S: Spec, Da: DaSpec> Sequencer<S, Da> {
    /// The total balance of the sequencer, including the bond and any additional balance.
    pub fn total_balance(&self) -> u64 {
        self.bond + self.additional_balance.unwrap_or(0)
    }
}

/// A convenient high-level representation of an optimistic genesis config. This config
/// is expressed in terms of abstract entities like Attesters and Sequencers, rather than
/// the low level details of accounts with balances held by several different modules.
///
/// This type can be converted into a low-level [`MinimalOptimisticGenesisConfig`] using
/// the [`From`] trait.
pub struct HighLevelOptimisticGenesisConfig<S: Spec, Da: DaSpec> {
    pub initial_attester: Attester<S>,
    pub initial_sequencer: Sequencer<S, Da>,
    pub additional_accounts: Vec<(S::Address, u64)>,
    pub gas_token_name: String,
}

impl<S: Spec, Da: DaSpec> HighLevelOptimisticGenesisConfig<S, Da> {
    /// Creates a new high-level genesis config with the given initial attester and sequencer using
    /// the default gas token name.
    pub fn with_defaults(
        initial_attester: Attester<S>,
        initial_sequencer: Sequencer<S, Da>,
    ) -> Self {
        Self {
            initial_attester,
            initial_sequencer,
            additional_accounts: vec![],
            gas_token_name: DEFAULT_GAS_TOKEN_NAME.to_string(),
        }
    }
}

impl HighLevelOptimisticGenesisConfig<TestSpec, MockDaSpec> {
    /// Generates a new high-level genesis config with random addresses and constant amounts (1_000_000_000 tokens).
    pub fn generate() -> Self {
        let attester = Attester {
            address: generate_address::<TestSpec>("attester"),
            bond: DEFAULT_BONDED_BALANCE,
            additional_balance: Some(DEFAULT_ADDITIONAL_BALANCE), // Give the attester extra tokens to pay for gas
        };
        let sequencer = Sequencer {
            rollup_address: generate_address::<TestSpec>("sequencer"),
            da_address: MockAddress::from([172; 32]),
            bond: DEFAULT_BONDED_BALANCE,
            additional_balance: Some(DEFAULT_ADDITIONAL_BALANCE),
        };
        Self::with_defaults(attester, sequencer)
    }
}

impl<S: Spec, Da: DaSpec> From<HighLevelOptimisticGenesisConfig<S, Da>>
    for MinimalOptimisticGenesisConfig<S, Da>
{
    fn from(high_level: HighLevelOptimisticGenesisConfig<S, Da>) -> Self {
        Self::from_args(
            high_level.initial_attester,
            high_level.initial_sequencer,
            high_level.additional_accounts.as_slice(),
            high_level.gas_token_name,
        )
    }
}

impl<S: Spec, Da: DaSpec> MinimalOptimisticGenesisConfig<S, Da> {
    pub fn from_args(
        initial_attester: Attester<S>,
        initial_sequencer: Sequencer<S, Da>,
        additional_accounts: &[(S::Address, u64)],
        gas_token_name: String,
    ) -> Self {
        Self {
            sequencer_registry: SequencerConfig {
                seq_rollup_address: initial_sequencer.rollup_address.clone(),
                seq_da_address: initial_sequencer.da_address.clone(),
                minimum_bond: initial_sequencer.bond,
                is_preferred_sequencer: true,
            },
            attester_incentives: AttesterIncentivesConfig {
                minimum_attester_bond: DEFAULT_MIN_USER_BOND,
                minimum_challenger_bond: DEFAULT_MIN_USER_BOND,
                initial_attesters: vec![(initial_attester.address.clone(), initial_attester.bond)],
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
                            (
                                initial_sequencer.rollup_address.clone(),
                                initial_sequencer.total_balance(),
                            ),
                            (
                                initial_attester.address.clone(),
                                initial_attester.total_balance(),
                            ),
                        ]);
                        additional_accounts_vec
                    },
                    authorized_minters: vec![],
                },
                tokens: vec![],
            },
        }
    }
}
