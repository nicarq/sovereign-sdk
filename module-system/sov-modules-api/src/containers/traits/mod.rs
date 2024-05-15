use sov_modules_core::namespaces::User;
use sov_modules_core::StateReaderAndWriter;

/// A type that can both read and write the normal "user-space" state of the rollup.
///
/// ```
/// fn delete_state_string(value: sov_modules_api::StateValue<String>, accessor: &mut impl sov_modules_api::StateAccessor) {
///     if let Some(original) = value.get(accessor) {
///         println!("original: {}", original);
///     }
///     value.delete(accessor);
/// }
///
///
/// ```
pub trait StateAccessor: StateReaderAndWriter<User> {}

impl<T> StateAccessor for T where T: StateReaderAndWriter<User> {}
