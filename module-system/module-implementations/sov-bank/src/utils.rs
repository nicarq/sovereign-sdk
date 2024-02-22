use sov_modules_api::digest::Digest;
use sov_modules_api::CryptoSpec;

use crate::genesis::DEPLOYER;

/// Derives token address from `token_name`, `sender` and `salt`.
pub fn get_token_address<S: sov_modules_api::Spec>(
    token_name: &str,
    sender: &S::Address,
    salt: u64,
) -> S::Address {
    let mut hasher = <S::CryptoSpec as CryptoSpec>::Hasher::new();
    hasher.update(sender.as_ref());
    hasher.update(token_name.as_bytes());
    hasher.update(salt.to_le_bytes());

    let hash: [u8; 32] = hasher.finalize().into();
    S::Address::from(hash)
}

/// Gets the token address for the genesis block using the `DEPLOYER` address as the sender.
pub fn get_genesis_token_address<S: sov_modules_api::Spec>(
    token_name: &str,
    salt: u64,
) -> S::Address {
    get_token_address::<S>(
        token_name,
        &S::Address::try_from(&DEPLOYER).expect("Illegal token deployer"),
        salt,
    )
}

#[cfg(feature = "test-utils")]
mod tests {
    use sov_modules_api::Spec;

    use crate::{Bank, BankGasConfig};

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
