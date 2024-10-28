use std::fmt;
use std::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_rollup_interface::da::DaSpec;

use crate::transaction::SequencerReward;
use crate::{InfallibleStateAccessor, Spec};

/// An error that can be returned within the [`SequencerAuthorization::authorize_sequencer`] capability.
pub struct AuthorizeSequencerError {
    /// The reason why the sequencer was not authorized.
    pub reason: anyhow::Error,
}

impl Debug for AuthorizeSequencerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AuthorizeSequencerError")
            .field(&self.reason)
            .finish()
    }
}

/// An allowed sequencer for a rollup.
#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize, Eq, PartialEq)]
#[serde(bound = "S::Address: serde::Serialize + serde::de::DeserializeOwned")]
pub struct AllowedSequencer<S: Spec> {
    /// The rollup address of the sequencer.
    pub address: S::Address,
    /// The staked balance of the sequencer.
    pub balance: u64,
}

/// Authorizes the sequencer to submit and process batches.
pub trait SequencerAuthorization<S: Spec> {
    /// Checks if the sequencer has staked the minimum bond to attest transactions.
    fn authorize_sequencer(
        &self,
        sequencer: &<<S as Spec>::Da as DaSpec>::Address,
        min_bond: u64,
        state: &mut impl InfallibleStateAccessor,
    ) -> Result<AllowedSequencer<S>, AuthorizeSequencerError>;
}

/// Functionality related to the rewarding and slashing of the sequencer.
pub trait SequencerRemuneration<S: Spec> {
    /// Reward the sequencer for correctly processing the transaction batch.
    /// This reward increases its staked balance.
    fn reward_sequencer(
        &self,
        sequencer: &<S::Da as DaSpec>::Address,
        reward: SequencerReward,
        state: &mut impl InfallibleStateAccessor,
    );

    /// Reward the sequencer for correctly processing the forced registration.
    /// If the registration was reverted, refund the sequencer rollup address.
    fn reward_sequencer_or_refund(
        &self,
        sequencer: &<S::Da as DaSpec>::Address,
        sequencer_rollup_address: &S::Address,
        reward: SequencerReward,
        state: &mut impl InfallibleStateAccessor,
    );
}
