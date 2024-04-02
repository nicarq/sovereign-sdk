//! Runtime call message definitions.

use sov_rollup_interface::maybestd::io;

use crate::common::ModuleError;
use crate::module::{CallResponse, Context, Spec};
use crate::storage::WorkingSet;
use crate::ModuleId;

/// A trait that needs to be implemented for any call message.
pub trait DispatchCall: Send + Sync {
    /// The context of the call
    type Spec: Spec;

    /// The concrete type that will decode into the call message of the module.
    type Decodable: Send + Sync;

    /// Decodes serialized call message
    fn decode_call(serialized_message: &[u8]) -> Result<Self::Decodable, io::Error>;

    /// Dispatches a call message to the appropriate module.
    fn dispatch_call(
        &self,
        message: Self::Decodable,
        working_set: &mut WorkingSet<Self::Spec>,
        context: &Context<Self::Spec>,
    ) -> Result<CallResponse, ModuleError>;

    /// Returns an address of the dispatched module.
    fn module_id(&self, message: &Self::Decodable) -> &ModuleId;
}
