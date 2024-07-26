use derive_getters::{Dissolve, Getters};
use derive_more::Constructor;
use sov_modules_api::{Module, Spec};

/// The [`PreparedCallMessage`] contains a module-specific call message,
/// the `max_fee` willing to be paid for the broadcasting of said call
/// message, as well as an account pool index which defines which account
/// in the account pool should sign this transaction.
#[derive(Debug, Constructor, Clone, Copy, Getters, Dissolve)]
pub struct PreparedCallMessage<S: Spec, M: Module<Spec = S>> {
    pub(crate) call_message: M::CallMessage,
    pub(crate) account_pool_index: u64,
    pub(crate) max_fee: u64,
}

/// The [`SerializedPreparedCallMessage`] is the same as the [`PreparedCallMessage`],
/// except in this case the `call_message` has been serialized to bytes per the spec
/// we're working with. This means no generics are required to define this structure.
#[derive(Default, Getters, Dissolve)]
pub struct SerializedPreparedCallMessage {
    pub(crate) call_message: Vec<u8>,
    pub(crate) account_pool_index: u64,
    pub(crate) max_fee: u64,
}
