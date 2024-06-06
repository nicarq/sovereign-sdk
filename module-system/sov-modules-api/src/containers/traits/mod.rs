use sov_state::namespaces::User;

use crate::StateReaderAndWriter;

/// A type that can both read and write the normal "user-space" state of the rollup.
///
/// ```
/// fn delete_state_string(value: sov_modules_api::StateValue<String>, state: &mut impl sov_modules_api::StateAccessor) {
///     if let Some(original) = value.get(state) {
///         println!("original: {}", original);
///     }
///     value.delete(state);
/// }
///
///
/// ```
pub trait StateAccessor: StateReaderAndWriter<User> {}

impl<T> StateAccessor for T where T: StateReaderAndWriter<User> {}
