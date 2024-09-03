use std::sync::atomic::AtomicU64;

use derive_getters::Getters;
use sov_cli::NodeClient;
use sov_modules_api::{PrivateKey, Spec};
use sov_rollup_interface::zk::CryptoSpec;

/// Represent where account has came from.
pub enum AccountOrigin {
    /// Externally sourced key
    Imported,
    /// Generated inside account pool, so we can reason about its state being not present in the rollup.
    Generated,
}

/// The [`Account`] structure holds a private key capable of signing transactions and it's corresponding
/// public address, per the spec we're working with. The nonce is also tracked in a thread safe way.
#[derive(Getters)]
pub struct Account<S: Spec> {
    nonce: AtomicU64,
    address: S::Address,
    private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    origin: AccountOrigin,
}

impl<S: Spec> Account<S> {
    pub(crate) fn new(
        private_key: <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
        origin: AccountOrigin,
    ) -> Self {
        let address: S::Address = (&private_key.pub_key()).into();
        Self {
            address,
            private_key,
            nonce: AtomicU64::default(),
            origin,
        }
    }

    fn generate_random() -> Self {
        Self::new(
            <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate(),
            AccountOrigin::Generated,
        )
    }

    pub(crate) fn generate_n_random(n: u64) -> Vec<Self> {
        (0..n).map(|_| Self::generate_random()).collect()
    }

    pub(crate) async fn refresh_nonce(&mut self, client: &NodeClient) -> anyhow::Result<()> {
        let nonce = client
            .get_nonce_for_public_key::<S>(&self.private_key.pub_key())
            .await?;

        self.nonce = nonce.into();

        Ok(())
    }
}
