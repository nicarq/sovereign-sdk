use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_macros::cycle_tracker;
use serde::{Deserialize, Serialize};
use sov_rollup_interface::digest::Digest;
use sov_rollup_interface::zk::CryptoSpec;
use tracing::debug;

use crate::transaction::Transaction;
use crate::Spec;

type RawTxHash = [u8; 32];

pub struct TransactionAndRawHash<S: Spec> {
    pub(crate) tx: Transaction<S>,
    pub(crate) raw_tx_hash: RawTxHash,
}

impl<S: Spec> TransactionAndRawHash<S> {
    pub fn split(self) -> (Transaction<S>, RawTxHash) {
        (self.tx, self.raw_tx_hash)
    }

    pub fn as_tuple(&self) -> (&Transaction<S>, &RawTxHash) {
        (&self.tx, &self.raw_tx_hash)
    }
}

/// RawTx represents a serialized rollup transaction received from the DA.
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub struct RawTx {
    /// Serialized transaction.
    pub data: Vec<u8>,
}

impl RawTx {
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn hash<S: Spec>(&self) -> [u8; 32] {
        <S::CryptoSpec as CryptoSpec>::Hasher::digest(&self.data).into()
    }

    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn deserialize<S: Spec>(&self) -> Result<Transaction<S>, std::io::Error> {
        Transaction::<S>::deserialize(&mut self.data.as_slice())
    }
}

pub fn verify_txs_stateless<S: Spec>(
    raw_txs: Vec<RawTx>,
) -> anyhow::Result<Vec<TransactionAndRawHash<S>>> {
    let mut txs = Vec::with_capacity(raw_txs.len());
    debug!(txs_num = raw_txs.len(), "Verifying transactions");
    for raw_tx in raw_txs {
        let raw_tx_hash = raw_tx.hash::<S>();
        let tx = raw_tx.deserialize()?;
        tx.verify()?;
        txs.push(TransactionAndRawHash { tx, raw_tx_hash });
    }
    Ok(txs)
}
