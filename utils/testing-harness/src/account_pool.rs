use std::collections::HashMap;
use std::path::Path;

use jsonrpsee::core::client::ClientT;
use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_modules_api::Spec;
use sov_nonces::NoncesRpcClient;
use sov_rollup_interface::crypto::{PrivateKey, PublicKey};
use sov_rollup_interface::zk::CryptoSpec;

#[derive(Debug, Clone)]
pub struct Account<S: Spec> {
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    pub nonce: u64,
}

impl<S: Spec> Account<S> {
    fn new_from_key(private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey) -> Self {
        Self {
            private_key,
            nonce: 0,
        }
    }
}

pub struct AccountPool<S: Spec> {
    known_keys: HashMap<S::Address, Account<S>>,
}

impl<S: Spec> AccountPool<S> {
    /// Populates account pool. Without nonces.
    pub fn from_keys_in_folder(dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = dir.as_ref();
        if !path.exists() {
            anyhow::bail!("Keys path {} does not exist", path.display());
        }
        if !path.is_dir() {
            anyhow::bail!("Keys path {} is not a folder", path.display());
        }

        let mut account_pool = AccountPool::<S> {
            known_keys: Default::default(),
        };

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

                        let account = Account {
                            private_key,
                            nonce: 0,
                        };

                        account_pool.known_keys.insert(address, account);
                    }
                    Err(e) => {
                        tracing::warn!(error = ?e, path = %path.display(), "Failed to parse key file, ignoring...");
                    }
                }
            }
        }

        Ok(account_pool)
    }

    /// If [`AccountPool`] has any known keys.
    pub fn is_empty(&self) -> bool {
        self.known_keys.is_empty()
    }

    pub fn contains_key(&self, address: &S::Address) -> bool {
        self.known_keys.contains_key(address)
    }

    pub fn addresses(&self) -> impl Iterator<Item = &S::Address> {
        self.known_keys.keys()
    }

    pub fn cycle_over_all(&self) -> impl Iterator<Item = &S::Address> {
        self.known_keys.keys().cycle()
    }

    /// Sync nonce for each account.
    pub async fn refresh_nonces(
        &mut self,
        client: &(impl ClientT + Send + Sync),
    ) -> anyhow::Result<()> {
        for account in self.known_keys.values_mut() {
            let credential_id = account
                .private_key
                .pub_key()
                .credential_id::<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>();

            let nonce = NoncesRpcClient::<S>::get_nonce(client, credential_id)
                .await?
                .nonce;
            account.nonce = nonce;
        }

        Ok(())
    }

    pub fn generate_new_key(&mut self) {
        let private_key = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate();
        let address: S::Address = (&private_key.pub_key()).into();
        tracing::debug!(%address, "New account been generated");
        self.known_keys
            .insert(address, Account::new_from_key(private_key));
    }

    pub fn get_mut_account(&mut self, address: &S::Address) -> Option<&mut Account<S>> {
        self.known_keys.get_mut(address)
    }
}
