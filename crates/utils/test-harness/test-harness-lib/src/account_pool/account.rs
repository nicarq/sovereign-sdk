use std::sync::atomic::AtomicU64;

use derive_getters::Getters;
use jsonrpsee::core::client::ClientT;
use sov_modules_api::{PrivateKey, PublicKey, Spec};
use sov_nonces::NoncesRpcClient;
use sov_rollup_interface::zk::CryptoSpec;

/// The [`Account`] structure holds a private key capable of signing transactions and it's corresponding
/// public address, per the spec we're working with. The nonce is also tracked in a thread safe way.
#[derive(Getters)]
pub struct Account<S: Spec> {
    nonce: AtomicU64,
    address: S::Address,
    private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
}

impl<S: Spec> Account<S> {
    pub(crate) fn new(private_key: <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey) -> Self {
        let address: S::Address = (&private_key.pub_key()).into();
        Self {
            address,
            private_key,
            nonce: AtomicU64::default(),
        }
    }

    fn generate_random() -> Self {
        Self::new(<<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate())
    }

    pub(crate) fn generate_n_random(n: u64) -> Vec<Self> {
        (0..n).map(|_| Self::generate_random()).collect()
    }

    pub(crate) async fn refresh_nonce(
        &mut self,
        client: &(impl ClientT + Send + Sync),
    ) -> anyhow::Result<()> {
        let credential_id = self
            .private_key
            .pub_key()
            .credential_id::<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>();

        let nonce = NoncesRpcClient::<S>::get_nonce(client, credential_id)
            .await?
            .nonce;

        self.nonce = nonce.into();

        Ok(())
    }
}
