use std::sync::Arc;

use sov_modules_api::capabilities::config_chain_id;
use sov_modules_api::prelude::arbitrary::{self, Unstructured};
use sov_modules_api::transaction::TxDetails;
use sov_modules_api::SafeVec;
use sov_paymaster::{PayeePolicy, PayerGenesisConfig, Paymaster, PaymasterConfig, PaymasterPolicy};
use sov_test_utils::runtime::genesis::optimistic::{
    HighLevelOptimisticGenesisConfig, MinimalOptimisticGenesisConfig,
};
use sov_test_utils::runtime::{TestRunner, ValueSetter, ValueSetterConfig};
use sov_test_utils::{
    generate_runtime, TestSequencer, TestSpec as S, TestUser, TransactionTestCase, TransactionType,
    TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE,
};
use sov_transaction_generator::generators::bank::harness_interface::BankHarness;
use sov_transaction_generator::generators::bank::BankMessageGenerator;
use sov_transaction_generator::generators::basic::{
    BasicCallMessageFactory, BasicChangeLogEntry, BasicClientConfig, BasicTag,
};
use sov_transaction_generator::generators::value_setter::{
    ValueSetterHarness, ValueSetterMessageGenerator, ValueSetterTag,
};
use sov_transaction_generator::interface::rng_utils::{get_random_bytes, randomize_buffer};
use sov_transaction_generator::interface::{MessageValidity, Percent};
use sov_transaction_generator::{
    AccountState, Distribution, GeneratedMessage, HarnessModule, State,
};
use sov_value_setter::CallMessageDiscriminants as ValueSetterDiscriminants;

mod combined;
mod successful_generation;
mod unsuccessful_generation;

const USER_BALANCE: u64 = 1_000_000_000_000;
const MAX_VEC_LEN_VALUE_SETTER: usize = 1000;

generate_runtime! {
    name: TestRuntime,
    modules: [paymaster: Paymaster<S>, value_setter: ValueSetter<S>],
    operating_mode: sov_modules_api::runtime::OperatingMode::Optimistic,
    minimal_genesis_config_type: MinimalOptimisticGenesisConfig<S>,
    gas_enforcer: paymaster: sov_paymaster::Paymaster<S>,
    runtime_trait_impl_bounds: [],
    kernel_type: sov_kernels::basic::BasicKernel<'a, S>
}

type RT = TestRuntime<S>;
type Generator = BasicCallMessageFactory<RT, S>;
type GeneratorOutput = GeneratedMessage<S, TestRuntimeCall<S>, BasicChangeLogEntry<S>>;

pub const BUFFER_SIZE: usize = 100_000;
// The minimum randomness needed to guarantee successful transaction generation
pub const SAFE_MIN_RANDOMNESS: usize = 1_000;
/// The number of transactions to generate.
pub const TXS_TO_GENERATE: u64 = 100;

enum ModulesToUse {
    Bank,
    ValueSetter,
}

impl ModulesToUse {
    /// Builds dynamic module reference
    #[allow(clippy::type_complexity)]
    pub fn select(
        &self,
        bank_harness: &BankHarness<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig, ()>,
        value_setter_harness: &ValueSetterHarness<
            S,
            RT,
            BasicTag,
            BasicChangeLogEntry<S>,
            BasicClientConfig,
            (),
        >,
    ) -> Arc<dyn HarnessModule<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig, ()>>
    {
        match self {
            ModulesToUse::Bank => {
                let module: Arc<
                    dyn HarnessModule<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig>,
                > = Arc::new(bank_harness.clone());
                module
            }
            ModulesToUse::ValueSetter => {
                let module: Arc<
                    dyn HarnessModule<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig>,
                > = Arc::new(value_setter_harness.clone());
                module
            }
        }
    }
}

struct NumTxsExecuted {
    num_bank_txs: u64,
    num_value_setter_txs: u64,
}

pub struct TestGenerator {
    generator: Generator,
    bank_harness: BankHarness<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig, ()>,
    value_setter_harness:
        ValueSetterHarness<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig, ()>,
    state: State<S, BasicTag>,
    randomness: Vec<u8>,
    remaining_randomness: usize,
    target_buffer_size: usize,
    salt: u128,
    initial_transaction: Option<GeneratorOutput>,
}

impl TestGenerator {
    #[allow(clippy::type_complexity)]
    pub fn generate(
        &mut self,
        modules_distribution: &Distribution<
            Arc<dyn HarnessModule<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig>>,
        >,
        validity: MessageValidity,
    ) -> GeneratorOutput {
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
fn setup_harness(
    address_creation_rate: Percent,
    admin: &TestUser<S>,
    max_value_setter_vec_len: usize,
    modules_distribution: &Distribution<ModulesToUse>,
) -> TestGenerator {
    use sov_bank::CallMessageDiscriminants::*;

    let bank_harness = BankHarness::new(BankMessageGenerator::<S>::new(
        Distribution::with_equiprobable_values(vec![Transfer, Freeze, Burn, Mint, CreateToken]),
        address_creation_rate,
    ));

    let value_setter_harness = ValueSetterHarness::new(ValueSetterMessageGenerator::<S>::new(
        Distribution::with_equiprobable_values(vec![
            ValueSetterDiscriminants::SetValue,
            ValueSetterDiscriminants::SetManyValues,
        ]),
        max_value_setter_vec_len,
    ));

    #[allow(clippy::type_complexity)]
    let modules: Vec<
        Arc<dyn HarnessModule<S, RT, BasicTag, BasicChangeLogEntry<S>, BasicClientConfig>>,
    > = modules_distribution
        .inner()
        .iter()
        .map(|(_, module_to_use)| module_to_use.select(&bank_harness, &value_setter_harness))
        .collect();

    let mut factory = Generator::new();

    // Synchronizes the state with the value setter module
    let mut state: State<S, BasicTag> = State::with_account_and_tags(
        AccountState {
            private_key: admin.private_key.clone(),
            balances: vec![],
            can_mint: Default::default(),
            sequencing_bond: None,
            additional_info: Default::default(),
        },
        vec![BasicTag::ValueSetter(ValueSetterTag::IsAdmin)],
    );

    let random_bytes: Vec<u8> = get_random_bytes(100_000, 0);
    let u = &mut arbitrary::Unstructured::new(&random_bytes[..]);
    let initial_tx = factory
        .generate_setup_messages(&modules, u, &mut state)
        .expect("Failed to generate setup messages")
        .pop();
    let remaining_randomness = u.len();
    TestGenerator {
        randomness: random_bytes,
        bank_harness,
        value_setter_harness,
        remaining_randomness,
        generator: factory,
        state,
        target_buffer_size: BUFFER_SIZE,
        salt: 1,
        initial_transaction: initial_tx,
    }
}

#[allow(unused)]
pub struct Setup {
    pub user: TestUser<S>,
    /// A user who is pre-registered as a payer for [`sequencer`]
    pub payer: TestUser<S>,
    /// The pre-registered sequencer
    pub sequencer: TestSequencer<S>,
    /// The admin user
    pub value_setter_admin: TestUser<S>,
    pub genesis_config: GenesisConfig<S>,
}

fn setup(user_balance: u64) -> Setup {
    let genesis_config = HighLevelOptimisticGenesisConfig::generate()
        .add_accounts_with_default_balance(2)
        .add_accounts_with_balance(2, user_balance);

    let sequencer = genesis_config.initial_sequencer.clone();
    let payer = genesis_config.additional_accounts.first().unwrap().clone();
    let admin = genesis_config.additional_accounts.get(1).unwrap().clone();
    let user = genesis_config.additional_accounts.get(2).unwrap().clone();
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
        ValueSetterConfig {
            admin: admin.address(),
        },
    );
    Setup {
        payer,
        sequencer,
        user,
        genesis_config,
        value_setter_admin: admin,
    }
}

async fn test_with_modules(
    modules: Distribution<ModulesToUse>,
    validity: Distribution<MessageValidity>,
    transaction_exec_closure: &mut impl FnMut(
        TransactionType<RT, S>,
        GeneratorOutput,
        &mut TestRunner<RT, S>,
    ),
) -> (TestRunner<RT, S>, TestGenerator, Vec<GeneratorOutput>) {
    let random_bytes = get_random_bytes(100_000_000, 1);
    let u = &mut Unstructured::new(&random_bytes[..]);

    let setup = setup(USER_BALANCE);
    let mut generator = setup_harness(
        Percent::one_hundred(),
        &setup.value_setter_admin,
        MAX_VEC_LEN_VALUE_SETTER,
        &modules,
    );

    let mut runner = TestRunner::<RT, S>::new_with_genesis(
        setup.genesis_config.into_genesis_params(),
        Default::default(),
    );

    // Execute initial transaction if there is one
    if let Some(init_tx) = generator.initial_transaction.take() {
        runner.execute_transaction(TransactionTestCase {
            input: TransactionType::Plain {
                message: init_tx.message.clone(),
                key: init_tx.sender.clone(),
                details: TxDetails {
                    max_priority_fee_bips: TEST_DEFAULT_MAX_PRIORITY_FEE,
                    max_fee: TEST_DEFAULT_MAX_FEE,
                    gas_limit: None,
                    chain_id: config_chain_id(),
                },
            },
            assert: Box::new(move |receipt, _state| {
                assert!(
                    receipt.tx_receipt.is_successful(),
                    "The initial transactions should always be successful"
                );
            }),
        });
    }

    let modules = modules.map_values(&mut |module| {
        module.select(&generator.bank_harness, &generator.value_setter_harness)
    });

    // Generate and execute [`TXS_TO_GENERATE`] txs
    let outputs: Vec<_> = (0..TXS_TO_GENERATE)
        .map(|_| {
            let validity = validity.select_value(u).expect("Ran out of randomness");
            let expected_output = generator.generate(&modules, *validity);

            let tx = TransactionType::Plain {
                message: expected_output.message.clone(),
                key: expected_output.sender.clone(),
                details: TxDetails {
                    max_priority_fee_bips: TEST_DEFAULT_MAX_PRIORITY_FEE,
                    max_fee: TEST_DEFAULT_MAX_FEE,
                    gas_limit: None,
                    chain_id: config_chain_id(),
                },
            };

            transaction_exec_closure(tx, expected_output.clone(), &mut runner);

            expected_output
        })
        .collect();

    (runner, generator, outputs)
}
