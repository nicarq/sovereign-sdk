use sov_modules_api::digest::Digest;
use sov_modules_api::CryptoSpec;

use crate::TokenId;

/// Derives token ID from `token_name`, `sender` and `salt`.
pub fn get_token_id<S: sov_modules_api::Spec>(
    token_name: &str,
    sender: &S::Address,
    salt: u64,
) -> TokenId {
    let mut hasher = <S::CryptoSpec as CryptoSpec>::Hasher::new();
    hasher.update(sender.as_ref());
    hasher.update(token_name.as_bytes());
    hasher.update(salt.to_le_bytes());

    let hash: [u8; 32] = hasher.finalize().into();
    TokenId::from(hash)
}

#[cfg(feature = "test-utils")]
mod tests {
    use sov_modules_api::digest::Digest;
    use sov_modules_api::{CryptoSpec, Spec};

    use crate::{Bank, BankGasConfig, TokenId};

    impl TokenId {
        /// Generates a deterministic token id by hashing the input string
        pub fn generate<S: Spec>(seed: &str) -> Self {
            let hash: [u8; 32] =
                <S::CryptoSpec as CryptoSpec>::Hasher::digest(seed.as_bytes()).into();
            hash.into()
        }
    }

    impl<S: Spec> Bank<S> {
        /// Returns the underlying gas config
        pub fn gas_config(&self) -> &BankGasConfig<S::Gas> {
            &self.gas
        }

        /// Overrides the underlying gas config
        pub fn override_gas_config(&mut self, gas: BankGasConfig<S::Gas>) {
            self.gas = gas;
        }
    }
}
