use sov_modules_api::capabilities::config_chain_id;
use sov_modules_api::prelude::{arbitrary, schemars, tokio};
use sov_modules_api::transaction::TxDetails;
use sov_paymaster::{
    PayeePolicy, PayerGenesisConfig, Paymaster, PaymasterConfig, PaymasterPolicy, SafeVec,
};
use sov_test_utils::runtime::genesis::optimistic::{
    HighLevelOptimisticGenesisConfig, MinimalOptimisticGenesisConfig,
};
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    generate_runtime, TestSequencer, TestSpec, TestUser, TransactionTestCase, TransactionType,
    TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE,
};
use sov_transaction_generator::generators::bank::{
    BankMessageGenerator, BankMessageGeneratorConfig,
};
use sov_transaction_generator::generators::basic::{
    BasicCallMessageGenerator, BasicCallMessageGeneratorConfig, BasicChangelogEntry,
    BasicClientConfig,
};
use sov_transaction_generator::interface::{
    CallMessageGenerator, Distribution, GeneratedMessage, MessageValidity, Percent,
};
use sov_transaction_generator::state::State;

use crate::{get_random_bytes, randomize_buffer};

generate_runtime! {
    name: TestRuntime,
    modules: [paymaster: Paymaster<S>],
    operating_mode: sov_modules_api::runtime::OperatingMode::Optimistic,
    minimal_genesis_config_type: MinimalOptimisticGenesisConfig<S>,
    impl_hooks: [SlotHooks, KernelSlotHooks, FinalizeHook, ApplyBatchHooks, TxHooks],
    gas_enforcer: paymaster: sov_paymaster::Paymaster<S>,
    runtime_trait_impl_bounds: [],
    kernel_type: sov_kernels::basic::BasicKernel<'a, S>
}

type RT = TestRuntime<TestSpec>;
type Generator = BasicCallMessageGenerator<RT, TestSpec>;
type GeneratorOutput =
    GeneratedMessage<TestSpec, TestRuntimeCall<TestSpec>, BasicChangelogEntry<TestSpec>>;

pub const BUFFER_SIZE: usize = 100_000;
// The minimum randomness needed to guarantee successful transaction generation
pub const SAFE_MIN_RANDOMNESS: usize = 1_000;

pub struct TestGenerator {
    generator: Generator,
    state: State<TestSpec, Generator>,
    randomness: Vec<u8>,
    remaining_randomness: usize,
    target_buffer_size: usize,
    salt: u128,
    initial_transaction: Option<GeneratorOutput>,
}

impl TestGenerator {
    pub fn generate(&mut self, validity: MessageValidity) -> GeneratorOutput {
        if let Some(tx) = self.initial_transaction.take() {
            return tx;
        }
        for _ in 0..20 {
            if self.has_enough_randomness() {
                let u =
                    &mut arbitrary::Unstructured::new(&self.randomness[self.randomness_offset()..]);
                if let Ok(output) =
                    self.generator
                        .generate_call_message(u, &mut self.state, validity)
                {
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
        self.salt += 1;
    }

    fn randomness_offset(&self) -> usize {
        self.randomness.len() - self.remaining_randomness
    }

    fn has_enough_randomness(&self) -> bool {
        self.remaining_randomness > std::cmp::min(SAFE_MIN_RANDOMNESS, self.target_buffer_size / 10)
    }
}

// Run generation with the given params
fn do_test(address_creation_rate: Percent) -> TestGenerator {
    use sov_bank::CallMessageDiscriminants::*;
    let bank_config = BankMessageGeneratorConfig {
        message_distribution: Distribution::with_equiprobable_values([
            Transfer,
            Transfer,
            Transfer,
            Mint,
            CreateToken,
        ]),
        address_creation_rate,
    };
    let bank_generator = BankMessageGenerator::<TestSpec>::from_config(bank_config.clone());
    let config = BasicCallMessageGeneratorConfig {
        module_distribution: Distribution::equiprobable(),
        bank: bank_config,
    };
    let mut generator = Generator::new(config, bank_generator);

    let mut state: State<TestSpec, Generator> = State::new();
    let random_bytes: Vec<u8> = get_random_bytes(100_000, 0);
    let u = &mut arbitrary::Unstructured::new(&random_bytes[..]);
    let initial_tx = generator.generate_initial_token(u, &mut state);
    let remaining_randomness = u.len();
    TestGenerator {
        randomness: random_bytes,
        remaining_randomness,
        generator,
        state,
        target_buffer_size: BUFFER_SIZE,
        salt: 1,
        initial_transaction: Some(initial_tx),
    }
}

#[allow(unused)]
pub struct Setup {
    pub user: TestUser<TestSpec>,
    /// A user who is pre-registered as a payer for [`sequencer`]
    pub payer: TestUser<TestSpec>,
    /// The pre-registered sequencer
    pub sequencer: TestSequencer<TestSpec>,
    pub genesis_config: GenesisConfig<TestSpec>,
}

fn setup(user_balance: u64) -> Setup {
    let genesis_config = HighLevelOptimisticGenesisConfig::generate()
        .add_accounts_with_default_balance(1)
        .add_accounts_with_balance(2, user_balance);

    let sequencer = genesis_config.initial_sequencer.clone();
    let payer = genesis_config.additional_accounts.first().unwrap().clone();
    let user = genesis_config.additional_accounts.get(1).unwrap().clone();
    let genesis_config = GenesisConfig::from_minimal_config(
        genesis_config.into(),
        PaymasterConfig {
            payers: [PayerGenesisConfig {
                payer_address: payer.address(),
                policy: PaymasterPolicy {
                    default_payee_policy: PayeePolicy::Allow {
                        max_fee: None,
                        gas_limit: None,
                        max_gas_price: None,
                    },
                    payees: SafeVec::new(),
                    authorized_sequencers: sov_paymaster::AuthorizedSequencers::All,
                    authorized_updaters: [payer.address()].as_ref().try_into().unwrap(),
                },
                sequencers_to_register: [sequencer.da_address].as_ref().try_into().unwrap(),
            }]
            .as_ref()
            .try_into()
            .unwrap(),
        },
    );
    Setup {
        payer,
        sequencer,
        user,
        genesis_config,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_successful_transaction_generation() {
    let setup = setup(1_000_000_000);
    let mut generator = do_test(Percent::one_hundred());

    let mut runner = TestRunner::<RT, TestSpec>::new_with_genesis(
        setup.genesis_config.into_genesis_params(),
        Default::default(),
    );

    let _ = runner.setup_rest_api_server().await;
    let config = BasicClientConfig {
        url: runner.base_path(),
        rollup_height: None,
    };
    // Generate and execute 100 txs
    for _ in 0..100 {
        let output = generator.generate(MessageValidity::Valid);
        let tx = TransactionType::Plain {
            message: output.message,
            key: output.sender,
            details: TxDetails {
                max_priority_fee_bips: TEST_DEFAULT_MAX_PRIORITY_FEE,
                max_fee: TEST_DEFAULT_MAX_FEE,
                gas_limit: None,
                chain_id: config_chain_id(),
            },
        };

        runner.execute_transaction(TransactionTestCase {
            input: tx,
            assert: Box::new(move |result, _state| {
                assert!(result.tx_receipt.is_successful(), "{:?}", result.tx_receipt);
            }),
        });
        generator
            .generator
            .assert_incremental_state(config.clone(), output.changes)
            .await
            .expect("State must be correct");
    }
}
