use std::path::PathBuf;

use sov_bank::Bank;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::capabilities::config_chain_id;
use sov_modules_api::prelude::arbitrary::{self};
use sov_modules_api::transaction::TxDetails;
use sov_modules_api::{Amount, DispatchCall, EncodeCall, Runtime};
use sov_paymaster::{
    PayeePolicy, PayerGenesisConfig, Paymaster, PaymasterConfig, PaymasterPolicyInitializer,
    SafeVec,
};
use sov_sequencer::preferred::PreferredSequencerConfig;
use sov_sequencer::SequencerKindConfig;
use sov_test_utils::runtime::genesis::zk::config::HighLevelZkGenesisConfig;
use sov_test_utils::runtime::genesis::zk::MinimalZkGenesisConfig;
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, TestRollup};
use sov_test_utils::{
    generate_runtime, RtAgnosticBlueprint, TestProver, TestSequencer, TestSpec as S, TestUser,
    TransactionType, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE,
    TEST_DEFAULT_USER_BALANCE,
};
use sov_transaction_generator::generators::basic::{
    BasicCallMessageFactory, BasicChangeLogEntry, BasicModuleRef, BasicTag,
};
use sov_transaction_generator::interface::rng_utils::{get_random_bytes, randomize_buffer};
use sov_transaction_generator::interface::MessageValidity;
use sov_transaction_generator::{Distribution, GeneratedMessage, State};

pub const DEFAULT_BLOCK_TIME_MS: u64 = 200;
pub const DEFAULT_BLOCK_PRODUCING_CONFIG: BlockProducingConfig = BlockProducingConfig::Periodic {
    block_time_ms: DEFAULT_BLOCK_TIME_MS,
};

pub const DEFAULT_FINALIZATION_BLOCKS: u32 = 5;

generate_runtime! {
    name: TestRuntime,
    modules: [paymaster: Paymaster<S>],
    operating_mode: sov_modules_api::runtime::OperatingMode::Zk,
    minimal_genesis_config_type: MinimalZkGenesisConfig<S>,
    gas_enforcer: paymaster: Paymaster<S>,
    runtime_trait_impl_bounds: [],
    kernel_type: sov_kernels::soft_confirmations::SoftConfirmationsKernel<'a, S>
}

pub type RT = TestRuntime<S>;

pub type RollupBlueprint = RtAgnosticBlueprint<S, RT>;
pub type TestRollupBuilder = RollupBuilder<RollupBlueprint, PathBuf>;

pub const BUFFER_SIZE: usize = 100_000;
// The minimum randomness needed to guarantee successful transaction generation
pub const SAFE_MIN_RANDOMNESS: usize = 1_000;

pub fn plain_tx_with_default_details<R: Runtime<S>>(
    gen_output: &GeneratedMessage<S, <R as DispatchCall>::Decodable, BasicChangeLogEntry<S>>,
) -> TransactionType<R, S> {
    TransactionType::Plain {
        message: gen_output.message.clone(),
        key: gen_output.sender.clone(),
        details: TxDetails {
            max_priority_fee_bips: TEST_DEFAULT_MAX_PRIORITY_FEE,
            max_fee: TEST_DEFAULT_MAX_FEE,
            gas_limit: None,
            chain_id: config_chain_id(),
        },
    }
}

pub struct TestGenerator<R: Runtime<S>> {
    generator: BasicCallMessageFactory<S, R>,
    state: State<S, BasicTag>,
    randomness: Vec<u8>,
    remaining_randomness: usize,
    target_buffer_size: usize,
    salt: u128,
}

impl<R: Runtime<S>> TestGenerator<R> {
    pub fn generate(
        &mut self,
        modules_distribution: &Distribution<BasicModuleRef<S, R>>,
        validity: MessageValidity,
    ) -> GeneratedMessage<S, <R as DispatchCall>::Decodable, BasicChangeLogEntry<S>> {
        for _ in 0..20 {
            if self.has_enough_randomness() {
                let u =
                    &mut arbitrary::Unstructured::new(&self.randomness[self.randomness_offset()..]);

                if let Ok(output) = self.generator.generate_call_message(
                    modules_distribution,
                    u,
                    &mut self.state,
                    validity,
                ) {
                    self.remaining_randomness = u.len();
                    return output;
                } else {
                    self.target_buffer_size *= 2;
                }
            }
            self.re_randomize();
        }
        unreachable!("Could not get enough randomness to generate a transaction");
    }

    fn re_randomize(&mut self) {
        if self.randomness.len() < self.target_buffer_size {
            self.randomness = vec![0; self.target_buffer_size];
        }
        randomize_buffer(&mut self.randomness[..], self.salt);
        self.remaining_randomness = self.randomness.len();
        self.salt += 1;
    }

    fn randomness_offset(&self) -> usize {
        self.randomness.len() - self.remaining_randomness
    }

    fn has_enough_randomness(&self) -> bool {
        self.remaining_randomness > std::cmp::min(SAFE_MIN_RANDOMNESS, self.target_buffer_size / 10)
    }
}

// Setup generation with the given params
pub fn setup_harness<R: Runtime<S> + EncodeCall<Bank<S>> + Clone>(
    rng_salt: u128,
) -> TestGenerator<R> {
    let factory = BasicCallMessageFactory::<S, R>::new();
    let state: State<S, BasicTag> = State::new();

    let random_bytes: Vec<u8> = get_random_bytes(100_000, rng_salt);
    let u = &mut arbitrary::Unstructured::new(&random_bytes[..]);
    let remaining_randomness = u.len();
    TestGenerator {
        randomness: random_bytes,
        remaining_randomness,
        generator: factory,
        state,
        target_buffer_size: BUFFER_SIZE,
        salt: rng_salt,
    }
}

pub struct Setup {
    /// A user who is pre-registered as a payer for [`Setup::sequencer`].
    #[allow(dead_code)]
    pub paymaster: TestUser<S>,
    /// The pre-registered sequencer
    pub sequencer: TestSequencer<S>,
    /// The pre-registered prover
    pub prover: TestProver<S>,
    #[allow(missing_docs)]
    pub genesis_config: GenesisConfig<S>,
}

fn setup_roles_and_config() -> Setup {
    let mut genesis_config = HighLevelZkGenesisConfig::generate();

    let sequencer = genesis_config.initial_sequencer.clone();
    let prover = genesis_config.initial_prover.clone();
    let paymaster = TestUser::generate(
        TEST_DEFAULT_USER_BALANCE
            .checked_mul(Amount::new(10))
            .unwrap(),
    );
    genesis_config.additional_accounts.push(paymaster.clone());

    let users: Vec<TestUser<S>> = vec![TestUser::generate_with_default_balance(); 20];

    genesis_config.additional_accounts.extend(users);
    let genesis_config = GenesisConfig::from_minimal_config(
        genesis_config.into(),
        PaymasterConfig {
            payers: [PayerGenesisConfig {
                payer_address: paymaster.address(),
                policy: PaymasterPolicyInitializer {
                    default_payee_policy: PayeePolicy::Allow {
                        max_fee: None,
                        gas_limit: None,
                        max_gas_price: None,
                        transaction_limit: None,
                    },
                    payees: SafeVec::new(),
                    authorized_sequencers: sov_paymaster::AuthorizedSequencers::All,
                    authorized_updaters: [paymaster.address()].as_ref().try_into().unwrap(),
                },
                sequencers_to_register: [sequencer.da_address].as_ref().try_into().unwrap(),
            }]
            .as_ref()
            .try_into()
            .unwrap(),
        },
    );
    Setup {
        paymaster,
        sequencer,
        prover,
        genesis_config,
    }
}

pub async fn setup_rollup(
    storage_path: PathBuf,
    axum_port: u16,
) -> (TestRollup<RollupBlueprint, PathBuf>, Setup) {
    let setup = setup_roles_and_config();
    let rollup_builder = TestRollupBuilder::new_with_storage_path(
        GenesisSource::CustomParams(setup.genesis_config.clone().into_genesis_params()),
        DEFAULT_BLOCK_PRODUCING_CONFIG,
        DEFAULT_FINALIZATION_BLOCKS,
        storage_path,
    )
    .set_config(|config| {
        config.telegraf_address = sov_metrics::MonitoringConfig::standard().telegraf_address;
        config.automatic_batch_production = true;
        config.rollup_prover_config = None;
        config.sequencer_config = SequencerKindConfig::Preferred(PreferredSequencerConfig {
            minimum_profit_per_tx: 0,
            ..Default::default()
        });
        config.prover_address = setup.prover.user_info.address().to_string();
        config.aggregated_proof_block_jump = 3;
        config.axum_port = axum_port;
    })
    .set_da_config(|da_config| {
        da_config.sender_address = setup.sequencer.da_address;
    });
    let rollup = rollup_builder
        .start()
        .await
        .expect("Impossible to start rollup");

    (rollup, setup)
}
