use derive_getters::Getters;
use derive_more::Constructor;
use sov_modules_api::Spec;

/// The configurables required to create an account pool.
#[derive(Getters, Constructor)]
pub struct AccountPoolConfig<S: Spec> {
    /// The directory that test-net, unencrypted private keys are stored in.
    private_keys_dir: String,

    /// The RPC url for the rollup. Used to query balances, nonces etc.
    rpc_url: String,

    /// How many random new user accounts to generate in the account pool.
    /// The total account pool includes this number ofd accounts plus the number
    /// of private keys that were parsed.
    num_accounts_to_generate: u64,

    /// A list of addresses who can mint the gas token of the rollup.
    gas_token_authorized_minters: Vec<S::Address>,
}
