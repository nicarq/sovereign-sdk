/// Derives the [`DispatchCall`] trait for the underlying
/// type.
#[cfg(feature = "macros")]
pub use sov_modules_macros::DispatchCall;
/// Derives the <runtime_name>Event enum for a given runtime.
#[cfg(feature = "macros")]
pub use sov_modules_macros::Event;
/// Derives the [`Genesis`](trait.Genesis.html) trait for the underlying runtime
/// `struct`.
#[cfg(feature = "macros")]
pub use sov_modules_macros::Genesis;
/// Derives the [`ModuleInfo`] trait for the underlying `struct`, giving full access to kernel functionality
#[cfg(feature = "macros")]
pub use sov_modules_macros::KernelModuleInfo;
#[cfg(feature = "macros")]
pub use sov_modules_macros::MessageCodec;
/// Derives the [`ModuleCallJsonSchema`](trait.ModuleCallJsonSchema.html) trait for
/// the underlying type.
///
/// ## Example
///
/// ```
/// use std::marker::PhantomData;
///
/// use sov_modules_api::{WorkingSet, ModuleId, Spec, Error, CallResponse, Context, Module, ModuleInfo, ModuleCallJsonSchema, StateMap};
/// use sov_test_utils::ZkTestSpec;
///
/// #[derive(ModuleInfo, ModuleCallJsonSchema)]
/// struct TestModule<S: Spec> {
///     #[id]
///     id: ModuleId,
///
///     #[state]
///     pub state_map: StateMap<String, u32>,
///
///     #[phantom]
///     phantom: std::marker::PhantomData<S>,
/// }
///
/// impl<S: Spec> Module for TestModule<S> {
///     type Spec = S;
///     type Config = PhantomData<S>;
///     type CallMessage = ();
///     type Event = ();
///     
///     fn call(
///        &self,
///        _msg: Self::CallMessage,
///        _context: &Context<Self::Spec>,
///        _working_set: &mut WorkingSet<S>,
///     ) -> Result<CallResponse, Error> {
///        Ok(CallResponse {})
///     }
/// }
///
/// println!("JSON Schema: {}", TestModule::<ZkTestSpec>::json_schema());
/// ```
#[cfg(feature = "macros")]
pub use sov_modules_macros::ModuleCallJsonSchema;
/// Derives the [`ModuleInfo`] trait for the underlying `struct`.
///
/// The underlying type must respect the following conditions, or compilation
/// will fail:
/// - It must be a named `struct`. Tuple `struct`s, `enum`s, and others are
/// not supported.
/// - It must have *exactly one* field with the `#[id]` attribute. This field
///   represents the **module id**.
/// - All other fields must have either the `#[state]` or `#[module]` attribute.
///   - `#[state]` is used for state members.
///   - `#[module]` is used for module members.
///
/// In addition to implementing [`ModuleInfo`], this macro will
/// also generate so-called "prefix" methods.
///
/// ## Example
///
/// ```
/// use sov_modules_api::{Spec, ModuleId, ModuleInfo, StateMap};
///
/// #[derive(ModuleInfo)]
/// struct TestModule<S: Spec> {
///     #[id]
///     id: ModuleId,
///
///     #[state]
///     pub state_map: StateMap<String, u32>,
///
///     #[phantom]
///     phantom: std::marker::PhantomData<S>,
/// }
///
/// // You can then get the prefix of `state_map` like this:
/// fn get_prefix<S: Spec>(some_storage: S::Storage) {
///     let test_struct = TestModule::<S>::default();
///     let prefix1 = test_struct.state_map.prefix();
/// }
/// ```
#[cfg(feature = "macros")]
pub use sov_modules_macros::ModuleInfo;

/// Procedural macros to assist with creating new modules.
#[cfg(feature = "macros")]
pub mod macros {
    /// Reads a string value from the rollup configuration manifest file and
    /// decodes it as a Bech32 value.
    ///
    /// The macro takes two arguments:
    ///  1. The name of the constant to be read from the manifest file, as a string literal.
    ///  2. The Bech32 newtype to decode the value into. This type must be
    ///     defined by [`impl_hash32_type`](crate::impl_hash32_type).
    pub use sov_modules_macros::config_bech32;
    /// Reads a JSON value from the rollup configuration manifest file and
    /// converts it into a Rust expression available at compile time. Nulls and
    /// objects are not supported.
    pub use sov_modules_macros::config_value;
    /// The macro exposes RPC endpoints from all modules in the runtime.
    /// It gets storage from the Context generic
    /// and utilizes output of [`#[rpc_gen]`] macro to generate RPC methods.
    ///
    /// It has limitations:
    ///   - First type generic attribute must have bound to [`Context`](sov_modules_core::Context) trait
    ///   - All generic attributes must own the data, thus have bound `'static`
    #[cfg(feature = "native")]
    pub use sov_modules_macros::expose_rpc;
    #[cfg(feature = "native")]
    pub use sov_modules_macros::rpc_gen;
    /// Implements the `sov_modules_api::CliWallet` trait for the annotated runtime.
    /// Under the hood, this macro generates an enum called `CliTransactionParser` which derives the [`clap::Parser`] trait.
    /// This enum has one variant for each field of the `Runtime`, and uses the `sov_modules_api::CliWalletArg` trait to parse the
    /// arguments for each of these structs.
    ///
    /// To exclude a module from the CLI, use the `#[cli_skip]` attribute.
    ///
    /// ## Examples
    /// ```
    /// use sov_modules_api::{Spec, DispatchCall, MessageCodec};
    /// use sov_modules_api::macros::CliWallet;
    ///
    /// #[derive(DispatchCall, MessageCodec, CliWallet)]
    /// #[serialization(borsh::BorshDeserialize, borsh::BorshSerialize)]
    /// pub struct Runtime<S: Spec> {
    ///     pub bank: sov_bank::Bank<S>,
    ///     // ...
    /// }
    /// ```
    #[cfg(feature = "native")]
    pub use sov_modules_macros::CliWallet;
    /// Implement [`CliWalletArg`](crate::CliWalletArg) for the annotated struct or enum. Unions are not supported.
    ///
    /// Under the hood, this macro generates a new struct or enum which derives the [`clap::Parser`] trait, and then implements the
    /// [`CliWalletArg`](crate::CliWalletArg) trait where the `CliStringRepr` type is the new struct or enum.
    ///
    /// As an implementation detail, `clap` requires that all types have named fields - so this macro auto generates an appropriate
    /// `clap`-compatible type from the annotated item. For example, the struct `MyStruct(u64, u64)` would be transformed into
    /// `MyStructWithNamedFields { field0: u64, field1: u64 }`.
    ///
    /// ## Example
    ///
    /// This code..
    /// ```rust
    /// use sov_modules_api::macros::CliWalletArg;
    /// #[derive(CliWalletArg, Clone)]
    /// pub enum MyEnum {
    ///    /// A number
    ///    Number(u32),
    ///    /// A hash
    ///    Hash { hash: String },
    /// }
    /// ```
    ///
    /// ...expands into the following code:
    /// ```rust,ignore
    /// // The original enum definition is left in its original place
    /// pub enum MyEnum {
    ///    /// A number
    ///    Number(u32),
    ///    /// A hash
    ///    Hash { hash: String },
    /// }
    ///
    /// // We generate a new enum with named fields which can derive `clap::Parser`.
    /// // Since this variant is only ever converted back to the original, we
    /// // don't carry over any of the original derives. However, we do preserve
    /// // doc comments from the original version so that `clap` can display them.
    /// #[derive(::clap::Parser)]
    /// pub enum MyEnumWithNamedFields {
    ///    /// A number
    ///    Number { field0: u32 } ,
    ///    /// A hash
    ///    Hash { hash: String },
    /// }
    /// // We generate a `From` impl to convert between the types.
    /// impl From<MyEnumWithNamedFields> for MyEnum {
    ///    fn from(item: MyEnumWithNamedFields) -> Self {
    ///       match item {
    ///         Number { field0 } => MyEnum::Number(field0),
    ///         Hash { hash } => MyEnum::Hash { hash },
    ///       }
    ///    }
    /// }
    ///
    /// impl sov_modules_api::CliWalletArg for MyEnum {
    ///     type CliStringRepr = MyEnumWithNamedFields;
    /// }
    /// ```
    #[cfg(feature = "native")]
    pub use sov_modules_macros::CliWalletArg;
    pub use sov_modules_macros::{address_type, offchain};
}
