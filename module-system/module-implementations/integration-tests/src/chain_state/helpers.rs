use sov_bank::{get_genesis_token_address, BankConfig, Coins, TokenConfig};
use sov_chain_state::ChainStateConfig;
use sov_modules_api::runtime::capabilities::Kernel;
use sov_modules_api::{DaSpec, Gas, GasArray, Spec};
use sov_modules_stf_blueprint::kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_modules_stf_blueprint::GenesisParams;
use sov_sequencer_registry::SequencerConfig;
use sov_test_utils::runtime::{GenesisConfig, TestRuntime};
use sov_value_setter::ValueSetterConfig;

pub(crate) fn create_chain_state_genesis_config<S: Spec, Da: DaSpec>(
    admin_pub_key: S::Address,
    seq_rollup_address: S::Address,
    seq_da_address: Da::Address,
    seq_stake_amount: u64,
    token_name: String,
    salt: u64,
    init_balance: u64,
) -> GenesisParams<GenesisConfig<S, Da>, BasicKernelGenesisConfig<S, Da>> {
    let runtime_config: <TestRuntime<S, Da> as sov_modules_stf_blueprint::Runtime<S, Da>>::GenesisConfig =
        GenesisConfig {
            value_setter: ValueSetterConfig { admin: admin_pub_key },
            sequencer_registry: SequencerConfig {
                seq_rollup_address: seq_rollup_address.clone(),
                seq_da_address,
                coins_to_lock: Coins { amount: seq_stake_amount, token_address: get_genesis_token_address::<S>(&token_name, salt) },
                is_preferred_sequencer: true,
            },
            bank: BankConfig {
                tokens: vec![TokenConfig {
                    token_name,
                    address_and_balances: vec![(seq_rollup_address.clone(), init_balance)],
                    authorized_minters: vec![seq_rollup_address.clone()],
                    salt,
                }]
            },
        };

    let kernel_config: <TestKernel<S, Da> as Kernel<S, Da>>::GenesisConfig =
        BasicKernelGenesisConfig {
            chain_state: ChainStateConfig {
                current_time: Default::default(),
                gas_price_blocks_depth: 10,
                gas_price_maximum_elasticity: 1,
                initial_gas_price: <<S::Gas as Gas>::Price as GasArray>::ZEROED,
                minimum_gas_price: <<S::Gas as Gas>::Price as GasArray>::ZEROED,
            },
        };
    GenesisParams {
        runtime: runtime_config,
        kernel: kernel_config,
    }
}

pub(crate) type TestKernel<S, Da> = BasicKernel<S, Da>;
