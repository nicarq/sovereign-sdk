//! This module defines the trait that is used to build batches of transactions.

use async_trait::async_trait;

use crate::TxHash;

/// [`BatchBuilder`] trait is responsible for managing a mempool and building
/// batches.
#[async_trait]
pub trait BatchBuilder: Sized + Send + Sync + 'static {
    /// Configuration values parsed from TOML for this [`BatchBuilder`].
    type Config: Send + Sync + 'static;

    /// Adds a **not-encoded** transaction to the mempool. The [`BatchBuilder`]
    /// implementation itself is responsible for "encoding" the transaction.
    ///
    /// Can return an error if transaction is invalid or mempool is full.
    async fn accept_tx(&mut self, tx: Vec<u8>) -> Result<TxWithHash, AcceptTxError>;

    /// Checks whether a transaction with the given `hash` is already in the
    /// mempool.
    async fn contains(&self, hash: &TxHash) -> anyhow::Result<bool>;

    /// Builds a new batch out of transactions in mempool.
    /// The logic of which transactions and how many of them are included in batch is up to implementation.
    async fn get_next_blob(&mut self, height: u64) -> anyhow::Result<Vec<TxWithHash>>;
}

/// Error type that can possibly arise during [`BatchBuilder::accept_tx`].
#[derive(Debug)]
pub struct AcceptTxError {
    /// The HTTP statuc code to return to the client.
    pub http_status: u16,
    /// Short, human-readable error message in English.
    pub title: String,
    /// Any additional information that might be useful for debugging. Will be sent to the client.
    pub details: String,
}

/// An encoded transaction with its hash as returned by
/// [`BatchBuilder::get_next_blob`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxWithHash {
    /// Encoded transaction.
    pub raw_tx: Vec<u8>,
    /// Transaction hash.
    pub hash: TxHash,
}
