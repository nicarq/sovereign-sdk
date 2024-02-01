/// A trait that enables event processing
pub trait RuntimeEventProcessor {
    /// Type specifying the wrapped enum for all events in the runtime
    type RuntimeEvent: borsh::BorshDeserialize
        + borsh::BorshSerialize
        + core::fmt::Debug
        + PartialEq
        + Into<sov_rollup_interface::rpc::EventResponse>;

    /// Function that converts module specific events to a wrapped event for storage
    fn convert_to_runtime_event(event: crate::storage::TypedEvent) -> Option<Self::RuntimeEvent>;
}
