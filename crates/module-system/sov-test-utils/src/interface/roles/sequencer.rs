use sov_modules_api::{CryptoSpec, DaSpec, Spec};

use super::StakedUser;

/// A representation of a sequencer at genesis.
#[derive(Debug, Clone)]
pub struct Sequencer<S: Spec, Da: DaSpec> {
    /// The private key of the sequencer.
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    /// The DA address of the sequencer.
    pub da_address: Da::Address,
    /// The amount of tokens to bond at genesis. These tokens will be minted by the bank.
    pub bond: u64,
    /// Any additional (not bonded) balance that the bank should mint for the attester.
    pub additional_balance: Option<u64>,
}

impl<S: Spec, Da: DaSpec> StakedUser<S> for Sequencer<S, Da> {
    /// Returns the private key of the sequencer.
    fn private_key(&self) -> &<S::CryptoSpec as CryptoSpec>::PrivateKey {
        &self.private_key
    }

    /// Returns the stake amount of the sequencer.
    fn bond(&self) -> u64 {
        self.bond
    }

    /// Returns the balance of the sequencer.
    fn free_balance(&self) -> u64 {
        self.additional_balance.unwrap_or(0)
    }
}

impl<S: Spec, Da: DaSpec> Sequencer<S, Da> {
    /// Returns the DA address of the sequencer.
    pub fn da_address(&self) -> &Da::Address {
        &self.da_address
    }
}
