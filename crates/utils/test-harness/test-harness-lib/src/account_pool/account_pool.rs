use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use derive_more::{Deref, DerefMut};
use jsonrpsee::http_client::HttpClientBuilder;
use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_modules_api::Spec;

use super::{Account, AccountPoolConfig};

/* */
/// The [`AccountPool`] is a structure holding a pool of accounts_from_disc that consist of:
///
/// - Existing accounts whose private keys are known, that are defined at rollup
///   genesis and which have some amount of the gas token required for the rollup.

/// - New, randomly generated accounts, the amount of which is defined by the
///   `--new-users-count` command line argument when running the harness.
///
/// Upon startup, the harness determines the balances of the first group of accounts
/// and mints new tokens to them should said balance not reach a certain threshold.
/// Next, the second set of randomly generated accounts are each apportioned a share
/// of that gas token so that they may take part in sending transactions to the rollup.
#[derive(Clone, Deref, DerefMut)]
pub struct AccountPool<S: Spec>(Arc<AccountPoolInner<S>>);

pub struct AccountPoolInner<S: Spec> {
    /// The index of an account in the [`ordered_accounts`] of this pool that is capable
    /// of minting the gas token of the rollup.
    gas_token_minter_index: u64,

    /// Since [`Account`] isn't `Ord` we use a BTreeMap with a `u64` key that is
    /// incremented by one for each addition to the map. This way we can iterate
    /// through the accounts efficiently when sending transactions from this pool.
    ordered_accounts: BTreeMap<u64, Account<S>>,

    /// The `ordered_accounts` `BTreeMap` herein makes it inefficient to get an
    /// [`Account`] by its address, so we also store here said account addresses
    /// and the index at which it exists in the `BTreeMap`. This allow for O(1)
    /// account lookups via address, but at the cost of having to store the
    /// address twice.
    ordered_accounts_indices: HashMap<S::Address, u64>,
}

impl<S: Spec> AccountPool<S> {
    pub(crate) fn gas_token_minter_index(&self) -> u64 {
        self.0.gas_token_minter_index
    }

    fn new_from_accounts(accounts: Vec<Account<S>>, gas_token_minter_index: u64) -> Self {
        let mut ordered_accounts_indices = HashMap::<S::Address, u64>::new();
        let mut ordered_accounts = BTreeMap::<u64, Account<S>>::new();
        accounts
            .into_iter()
            .enumerate()
            .for_each(|(index, account)| {
                ordered_accounts_indices.insert(account.address().clone(), index as u64);
                ordered_accounts.insert(index as u64, account);
            });
        Self(Arc::new(AccountPoolInner {
            ordered_accounts,
            gas_token_minter_index,
            ordered_accounts_indices,
        }))
    }

    /// Increments the nonce by one of the account at the passed in index.
    pub fn inc_nonce(&self, index: &u64) {
        self.ordered_accounts
            .get(index)
            .expect("could not get account at index {index}")
            .nonce()
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Gets an iterator of all the public addresses held in the account pool.
    pub fn addresses(&self) -> impl Iterator<Item = &S::Address> + std::clone::Clone {
        self.ordered_accounts
            .values()
            .map(|account| account.address())
    }

    fn from_keys_in_folder(dir: impl AsRef<Path>) -> anyhow::Result<Vec<Account<S>>> {
        let path = dir.as_ref();
        if !path.exists() {
            anyhow::bail!("Keys path {} does not exist", path.display());
        }
        if !path.is_dir() {
            anyhow::bail!("Keys path {} is not a folder", path.display());
        }

        let mut existing_keys = Vec::<Account<S>>::new();

        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                // TODO: Collect sub dirs
                tracing::warn!("SKIP DIR");
            } else {
                match PrivateKeyAndAddress::<S>::from_json_file(&path) {
                    Ok(PrivateKeyAndAddress {
                        private_key,
                        address,
                    }) => {
                        tracing::info!(address = %address, path = %path.display(), "Parsed key file");

                        let account = Account::new(private_key);

                        existing_keys.push(account);
                    }
                    Err(e) => {
                        tracing::warn!(error = ?e, path = %path.display(), "Failed to parse key file, ignoring...");
                    }
                }
            }
        }

        Ok(existing_keys)
    }

    /// Create an account pool from the [`AccountPoolConfig`], which defines where a set of genesis test
    /// private keys may be found. These keys are assumed to have been given X of the rollups gas token
    /// at genesis, so as to be able to make transactions.
    pub async fn new_from_config(config: AccountPoolConfig<S>) -> anyhow::Result<Self> {
        let mut accounts = Self::from_keys_in_folder(config.private_keys_dir())?;
        let addresses_read_from_disc = accounts
            .iter()
            .map(|account| account.address())
            .cloned()
            .collect::<Vec<_>>();

        if accounts.is_empty() {
            anyhow::bail!("Cannot proceed without any known key");
        }

        for account in &accounts {
            tracing::debug!(address = %account.address(), "Address has been read from disk");
        }

        let client = HttpClientBuilder::default().build(config.rpc_url())?;

        // TODO @gskapka pass in a flag to opt out of this if you know the accounts from file have a nonce of 0?
        // Refreshing nonces before generating new users to avoid non needed RPC calls.
        for account in &mut accounts {
            account.refresh_nonce(&client).await?;
        }

        accounts.append(&mut Account::generate_n_random(
            *config.num_accounts_to_generate(),
        ));

        let mut maybe_minter_index = None;
        for (account_pool_index, address) in addresses_read_from_disc.into_iter().enumerate() {
            if maybe_minter_index.is_none()
                && config.gas_token_authorized_minters().contains(&address)
            {
                maybe_minter_index = Some(account_pool_index);
            }
        }

        let gas_token_minter_index = if let Some(index) = maybe_minter_index {
            index as u64
        } else {
            anyhow::bail!("cannot proceed withough an account that can mint the rollup gas token!");
        };

        let account_pool = AccountPool::new_from_accounts(accounts, gas_token_minter_index);

        Ok(account_pool) // TODO return tx batch(es) too so they can go to the blob submitter?
    }

    /// Returns whether or not a given address exists in this [`AccountPool`].
    pub fn contains_address(&self, address: &S::Address) -> bool {
        self.ordered_accounts_indices.contains_key(address)
    }

    /// Gets an [`Account`] from this [`AccountPool`] by its index in the `BTreeMap`
    pub fn get_by_index(&self, index: &u64) -> Option<&Account<S>> {
        self.ordered_accounts.get(index)
    }

    /// Gets an [`sov_modules_api::Spec::Address`] from this [`AccountPool`] by its index in the `BTreeMap`
    pub fn get_address_by_index(&self, index: &u64) -> Option<&S::Address> {
        self.ordered_accounts
            .get(index)
            .map(|account| account.address())
    }

    #[allow(unused)]
    /// Gets an [`Account`] from this [`AccountPool`] by its address if extant.
    pub fn get_by_address(&self, address: &S::Address) -> Option<&Account<S>> {
        if let Some(index) = self.get_index(address) {
            self.ordered_accounts.get(index)
        } else {
            None
        }
    }

    /// Returns how many [`crate::account_pool::account::Account`]s exist in this [`AccountPool`].
    pub fn len(&self) -> usize {
        self.ordered_accounts.len()
    }

    /// Whether or not this [`AccountPool`] contains any [`Account`]s.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Gets an [`Account`]'s index from this [`AccountPool`] via the account's address.
    pub fn get_index(&self, address: &S::Address) -> Option<&u64> {
        self.ordered_accounts_indices.get(address)
    }
}
