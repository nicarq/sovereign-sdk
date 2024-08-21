use sov_modules_api::{CryptoSpec, Module, PrivateKey, Spec};

mod attester_incentives;
mod prover;
mod sequencer;

pub use attester_incentives::*;
pub use prover::{TestProver, TestProverConfig};
pub use sequencer::{TestSequencer, TestSequencerConfig};

use super::TransactionType;
use crate::default_test_tx_details;
use crate::runtime::genesis::TestTokenName;

/// A minimal representation of a token held by a given user.
#[derive(Debug, Clone)]
pub struct UserTokenInfo {
    /// The associated token name
    pub token_name: TestTokenName,
    /// The user balance
    pub balance: u64,
    /// If the user can mint the token
    pub is_minter: bool,
}

/// A representation of a simple user that is not staked at genesis.
#[derive(Debug, Clone)]
pub struct TestUser<S: Spec> {
    /// The private key of the user.
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    /// The bank balance of the user for the default gas token.
    pub available_gas_balance: u64,
    /// The balances of the user for each non-gas token.
    pub token_balances: Vec<UserTokenInfo>,
}

impl<S: Spec> TestUser<S> {
    /// Creates a new user with the given private key and balance.
    pub fn new(private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey, balance: u64) -> Self {
        Self {
            private_key,
            available_gas_balance: balance,
            token_balances: Vec::new(),
        }
    }

    /// Generates a new user with the given balance.
    pub fn generate(balance: u64) -> Self {
        Self {
            private_key: <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate(),
            available_gas_balance: balance,
            token_balances: Vec::new(),
        }
    }

    /// Adds a balance to the user for the given test token.
    pub fn add_token_info(mut self, info: UserTokenInfo) -> Self {
        self.token_balances.push(info);

        self
    }

    /// Returns the address of the user.
    pub fn address(&self) -> <S as Spec>::Address {
        <S as Spec>::Address::from(&self.private_key.pub_key())
    }

    /// Returns the private key of the user.
    pub fn private_key(&self) -> &<S::CryptoSpec as CryptoSpec>::PrivateKey {
        &self.private_key
    }

    /// Returns the balance of the user.
    pub fn balance(&self) -> u64 {
        self.available_gas_balance
    }

    /// Returns the balance of the user for the given token.
    pub fn token_balance(&self, token_name: &TestTokenName) -> Option<u64> {
        self.token_balances
            .iter()
            .find(|info| info.token_name == *token_name)
            .map(|info| info.balance)
    }

    /// Returns true if the user is a minter for the given token.
    pub fn is_minter(&self, token_name: &TestTokenName) -> bool {
        self.token_balances
            .iter()
            .find(|info| info.token_name == *token_name)
            .map(|info| info.is_minter)
            .unwrap_or(false)
    }
}

impl<S: Spec> AsUser<S> for TestUser<S> {
    fn as_user(&self) -> &TestUser<S> {
        self
    }

    fn as_user_mut(&mut self) -> &mut TestUser<S> {
        self
    }
}

/// A trait that can be used to convert a special into a [`TestUser`] struct.
pub trait AsUser<S: Spec> {
    /// Returns a reference to an underlying [`TestUser`].
    fn as_user(&self) -> &TestUser<S>;

    /// Returns a mutable reference to an underlying [`TestUser`].
    fn as_user_mut(&mut self) -> &mut TestUser<S>;

    /// Creates a plain message from the user.
    fn create_plain_message<M: Module<Spec = S>>(
        &self,
        message: M::CallMessage,
    ) -> TransactionType<M, S> {
        TransactionType::Plain {
            message,
            key: self.as_user().private_key().clone(),
            details: default_test_tx_details::<S>(),
        }
    }
}
