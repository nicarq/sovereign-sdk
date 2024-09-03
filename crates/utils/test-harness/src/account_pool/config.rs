use derive_getters::Getters;
use derive_more::Constructor;

/// The configurables required to create an account pool.
#[derive(Getters, Constructor)]
pub struct AccountPoolConfig {
    /// The directory that test-net, unencrypted private keys are stored in.
    private_keys_dir: String,

    /// The REST API URL of the rollup node. Used to query balances, nonces etc.
    node_url: String,

    /// How many random new user accounts to generate in the account pool.
    /// The total account pool includes this number ofd accounts plus the number
    /// of private keys that were parsed.
    num_accounts_to_generate: u64,
}
