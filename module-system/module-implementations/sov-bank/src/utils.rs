use borsh::BorshDeserialize;
use sov_modules_api::digest::Digest;
use sov_modules_api::{CryptoSpec, ModuleId, Spec};
use sov_state::codec::{BcsCodec, BorshCodec, JsonCodec};
use sov_state::storage::EncodeKeyLike;

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

/// An identifier which can hold tokens on the rollup. This is implemented by `&S::Address`. To pay a module,
/// make sure the `AsPayable` trait is in scope, and call `module_id.to_payable()`.
///
/// When a function accepts `impl Payable<S>` as an argument, you can pass `S::Address` or `ModuleId`, or (to avoid copying)
/// `module_id.as_token_holder()`
pub trait Payable<S: Spec>: std::fmt::Display {
    /// Converts the identifier into a standard format understood by the bank module.
    fn as_token_holder(&self) -> TokenHolderRef<'_, S>;
}

/// A type which can be converted to a type that implements `Payable<S>`. Usually a `ModuleId`.
pub trait IntoPayable<S: Spec> {
    /// A type which implements `Payable<S>` that can be constructed from the current type.
    type Output<'a>: Payable<S>
    where
        Self: 'a;
    /// Converts the
    fn to_payable(&self) -> Self::Output<'_>;
}

impl<S: Spec> Payable<S> for &S::Address {
    fn as_token_holder(&self) -> TokenHolderRef<'_, S> {
        TokenHolderRef::User(*self)
    }
}

impl<S: Spec> IntoPayable<S> for ModuleId {
    type Output<'a> = TokenHolderRef<'a, S>;
    fn to_payable(&self) -> TokenHolderRef<'_, S> {
        TokenHolderRef::Module(self)
    }
}

impl<S: Spec> Payable<S> for TokenHolder<S> {
    fn as_token_holder(&self) -> TokenHolderRef<'_, S> {
        match self {
            Self::User(addr) => TokenHolderRef::User(addr),
            Self::Module(id) => TokenHolderRef::Module(id),
        }
    }
}

impl<'a, S: Spec> Payable<S> for TokenHolderRef<'a, S> {
    fn as_token_holder(&self) -> TokenHolderRef<'a, S> {
        *self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, BorshDeserialize)]
/// The identifier of a a payable entity on the rollup. This can be either a user or a module.
pub enum TokenHolder<S: Spec> {
    /// A external address the rollup.
    User(S::Address),
    /// A builtin module.
    Module(ModuleId),
}

impl<Sp: Spec> serde::Serialize for TokenHolder<Sp> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let id_ref: TokenHolderRef<'_, Sp> = self.into();
        id_ref.serialize(serializer)
    }
}

impl<S: Spec> borsh::BorshSerialize for TokenHolder<S> {
    fn serialize<W: std::io::prelude::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let id_ref: TokenHolderRef<'_, S> = self.into();
        borsh::BorshSerialize::serialize(&id_ref, writer)
    }
}

impl<S: Spec> std::fmt::Display for TokenHolder<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenHolder::User(addr) => write!(f, "{}", addr),
            TokenHolder::Module(id) => write!(f, "{}", id),
        }
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, borsh::BorshSerialize)]
/// A reference to a payable entity on the rollup. This can be either a user or a module.
pub enum TokenHolderRef<'a, S: Spec> {
    /// A reference to a user's address
    User(&'a S::Address),
    /// A reference to a module's ID
    Module(&'a ModuleId),
}

// Manually implement Clone because derive infurs a spurious `Spec: Clone` bound
impl<'a, S: Spec> Clone for TokenHolderRef<'a, S> {
    fn clone(&self) -> Self {
        *self
    }
}

// Manually implement Copy because derive infurs a spurious `Spec: Copy` bound
impl<S: Spec> Copy for TokenHolderRef<'_, S> {}

impl<S: Spec> std::fmt::Display for TokenHolderRef<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenHolderRef::User(addr) => write!(f, "{}", addr),
            TokenHolderRef::Module(id) => write!(f, "{}", id),
        }
    }
}

impl<'a, S: Spec> From<&'a TokenHolder<S>> for TokenHolderRef<'a, S> {
    fn from(item: &'a TokenHolder<S>) -> TokenHolderRef<'a, S> {
        match item {
            TokenHolder::User(addr) => TokenHolderRef::User(addr),
            TokenHolder::Module(id) => TokenHolderRef::Module(id),
        }
    }
}

// use the autoref trick to prevent conflicts since rustc doesn't know that S::Address
// cannot be the same type as ModuleId
impl<'a, S: Spec> From<&&'a S::Address> for TokenHolderRef<'a, S> {
    fn from(value: &&'a S::Address) -> Self {
        Self::User(*value)
    }
}

impl<'a, S: Spec> From<&'a ModuleId> for TokenHolderRef<'a, S> {
    fn from(value: &'a ModuleId) -> Self {
        Self::Module(value)
    }
}

// Implement the `encode_key_like` trait, marking for Rustc that TokenHolderRef and TokenHolder can be serialized
// identically for all of our supported codecs
mod encode_key_like {
    use sov_state::storage::StateItemEncoder;

    use super::*;

    impl<S: Spec> EncodeKeyLike<TokenHolderRef<'_, S>, TokenHolder<S>> for BcsCodec {
        fn encode_key_like(&self, borrowed: &TokenHolderRef<'_, S>) -> Vec<u8> {
            self.encode(borrowed)
        }
    }

    impl<S: Spec> EncodeKeyLike<TokenHolderRef<'_, S>, TokenHolder<S>> for JsonCodec {
        fn encode_key_like(&self, borrowed: &TokenHolderRef<'_, S>) -> Vec<u8> {
            self.encode(borrowed)
        }
    }

    impl<S: Spec> EncodeKeyLike<TokenHolderRef<'_, S>, TokenHolder<S>> for BorshCodec {
        fn encode_key_like(&self, borrowed: &TokenHolderRef<'_, S>) -> Vec<u8> {
            self.encode(borrowed)
        }
    }
}
