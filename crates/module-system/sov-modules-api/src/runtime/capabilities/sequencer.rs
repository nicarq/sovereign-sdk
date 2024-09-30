use std::fmt;
use std::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_rollup_interface::da::DaSpec;

use crate::transaction::SequencerReward;
use crate::{Gas, GasMeter, PreExecWorkingSet, Spec, TxScratchpad};

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

/// The result of the [`SequencerAuthorization::authorize_sequencer`] capability.
pub type AuthorizationResult<S, SequencerStakeMeter> =
    Result<(AllowedSequencer<S>, SequencerStakeMeter), AuthorizeSequencerError>;

/// Authorizes the sequencer to submit and process batches.
pub trait SequencerAuthorization<S: Spec, Da: DaSpec> {
    /// A type-safe struct that should track the staked amount of the sequencer and the eventual execution penalities.
    type SequencerStakeMeter: GasMeter<S::Gas>;

    /// Checks if the sequencer has staked the minimum bond to attest transactions.
    ///
    /// ## Returns
    /// Returns a [`AuthorizeSequencerError`] error if the sequencer is not registered or does not have enough staked amount.
    /// Returns a `Self::SequencerStakeMeter` if the sequencer is registered and has enough staked amount.
    fn authorize_sequencer(
        &self,
        sequencer: &Da::Address,
        base_fee_per_gas: &<S::Gas as Gas>::Price,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> AuthorizationResult<S, Self::SequencerStakeMeter>;

    /// Penalizes the sequencer without slashing his account.
    /// If the sequencer is penalized, the stake amount of the sequencer is reduced, potentially preventing future transactions from being executed.
    ///
    /// ## Note
    /// This method consumes the [`PreExecWorkingSet`].
    /// It should only be called once the sequencer cannot be penalized anymore.
    /// The penalty should be accumulated in the [`SequencerAuthorization::SequencerStakeMeter`] during the execution of the transaction.
    fn penalize_sequencer(
        &self,
        sequencer: &Da::Address,
        reason: impl std::fmt::Display,
        pre_exec_ws: PreExecWorkingSet<S, Self::SequencerStakeMeter>,
    ) -> TxScratchpad<S::Storage>;
}

/// Functionality related to the rewarding and slashing of the sequencer.
pub trait SequencerRemuneration<S: Spec, Da: DaSpec> {
    /// Reward the sequencer for correctly processing the transaction batch.
    fn reward_sequencer(
        &self,
        sender: &S::Address,
        reward: SequencerReward,
        state: &mut TxScratchpad<S::Storage>,
    );

    /// Slash the sequencer for malicious behavior.
    fn slash_sequencer(&self, sender: &Da::Address, state: &mut TxScratchpad<S::Storage>);
}
