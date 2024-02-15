/// A trait that enables event processing for storage
pub trait RuntimeEventProcessor {
    /// Type specifying the wrapped enum for all events in the runtime
    type RuntimeEvent: borsh::BorshDeserialize
        + borsh::BorshSerialize
        + core::fmt::Debug
        + PartialEq;

    /// Function that converts module specific events to a wrapped event for storage
    fn convert_to_runtime_event<Co: crate::Context>(
        event: crate::storage::TypedEvent<Co>,
    ) -> Option<Self::RuntimeEvent>;
}

/// A trait that enables event display from storage in a human readable format
#[cfg(feature = "native")]
pub trait RuntimeEventDisplay {
    /// Type specifying the wrapped enum for all events in the runtime
    type RuntimeEvent: borsh::BorshDeserialize
        + borsh::BorshSerialize
        + core::fmt::Debug
        + PartialEq
        + Into<sov_rollup_interface::rpc::Event>;
}
