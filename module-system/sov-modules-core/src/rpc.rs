//! Defines traits for interacting with the ledger when using the sov module system.
use async_trait::async_trait;
use sov_rollup_interface::rpc::{Event, LedgerStateProvider, PaginatedEventResponse};

use crate::ModuleId;

/// A [`LedgerStateProviderExt`] provides a way to query the ledger for events by module.
#[async_trait]
pub trait LedgerStateProviderExt: LedgerStateProvider {
    /// Get events by key.
    async fn get_events_by_key<E: borsh::BorshDeserialize + Into<Event>>(
        &self,
        event_key: &str,
        module_id: Option<ModuleId>,
        txn_range: Option<(u64, u64)>,
        num_events: usize,
        next: Option<&str>,
    ) -> Result<PaginatedEventResponse, Self::Error>;

    /// Get events by module id
    async fn get_events_by_module_id<E: borsh::BorshDeserialize + Into<Event>>(
        &self,
        module_id: ModuleId,
        num_events: usize,
        next: Option<&str>,
    ) -> Result<PaginatedEventResponse, Self::Error>;

    /// Get events by a range of slots and key.
    async fn get_events_by_slot_range_key<E: borsh::BorshDeserialize + Into<Event>>(
        &self,
        event_key: &str,
        module_id: ModuleId,
        slot_height_start: u64,
        slot_height_end: u64,
        num_events: usize,
        next: Option<&str>,
    ) -> Result<PaginatedEventResponse, Self::Error>;
}
