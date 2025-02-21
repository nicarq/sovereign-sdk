use std::convert::Infallible;
use std::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_rollup_interface::common::VisibleSlotNumber;
use sov_rollup_interface::da::DaSpec;
use sov_state::{Kernel, User};

use crate::transaction::SequencerReward;
use crate::{InfallibleStateAccessor, Spec, StateReader, StateWriter};

/// The status of the sequencer's balance.
#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize, Eq, PartialEq)]
pub enum BalanceState {
    /// The sequencer has enough balance to submit and process batches.
    Active,
    /// The sequencer has insufficient balance to submit and process batches.
    PendingWithdrawal {
        /// The slot number at which the sequencer will be able to withdraw.
        ready_at: VisibleSlotNumber,
    },
}

impl BalanceState {
    /// Returns true if the sequencer is active.
    pub fn is_active(&self) -> bool {
        matches!(self, BalanceState::Active)
    }

    /// Returns true if the sequencer is pending withdrawal.
    pub fn is_pending_withdrawal(&self) -> bool {
        matches!(self, BalanceState::PendingWithdrawal { .. })
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
    /// The balance state of the sequencer.
    pub balance_state: BalanceState,
}

/// Authorizes the sequencer to submit and process batches.
pub trait SequencerAuthorization<S: Spec> {
    /// Authorize the preferred sequencer based on their current balance.
    fn is_preferred_sequencer(
        &self,
        sequencer: &<<S as Spec>::Da as DaSpec>::Address,
        state: &mut impl InfallibleStateAccessor,
    ) -> bool;
}

/// Functionality related to the rewarding and slashing of the sequencer.
pub trait SequencerRemuneration<S: Spec> {
    /// Reward the sequencer for correctly processing the forced registration.
    /// If the registration was reverted, refund the sequencer rollup address.
    fn reward_sequencer_or_refund<
        Accessor: StateReader<Kernel, Error = Infallible>
            + StateWriter<Kernel, Error = Infallible>
            + StateWriter<User, Error = Infallible>
            + StateReader<User, Error = Infallible>,
    >(
        &self,
        sequencer: &<S::Da as DaSpec>::Address,
        sequencer_rollup_address: &S::Address,
        reward: SequencerReward,
        state: &mut Accessor,
    );

    /// Gets the address of the preferred sequencer, if one exists.
    fn preferred_sequencer(
        &self,
        scratchpad: &mut impl InfallibleStateAccessor,
    ) -> Option<<S::Da as DaSpec>::Address>;
}
