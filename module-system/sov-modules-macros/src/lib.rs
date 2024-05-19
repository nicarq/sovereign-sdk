//! Procedural macros to assist in the creation of Sovereign modules.
//!
//! This crate is not intended to be used directly, please refer to the
//! documentation of [`sov_modules_api`](https://docs.rs/sov-modules-api) for
//! more information with the `macros` feature flag.

// This crate is `missing_docs` because it is not intended to be used directly,
// but only through the re-exports in `sov_modules_api`. All re-exports are
// documented there.
#![allow(missing_docs)]

#[cfg(feature = "native")]
mod cli_parser;
mod common;
mod compile_manifest_constants;
mod dispatch;
mod event;
mod expand_macro;
mod manifest;
mod module_info;
mod new_types;
mod offchain;
#[cfg(feature = "native")]
mod rpc;

use compile_manifest_constants::{make_const_bech32, make_const_value};
use dispatch::dispatch_call::DispatchCallMacro;
use dispatch::genesis::GenesisMacro;
use dispatch::message_codec::MessageCodec;
use event::EventMacro;
use module_info::ModuleType;
use new_types::address_type_helper;
use offchain::offchain_generator;
use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput, ItemFn};

/// Returns the name of the function that invoked the proc-macro.
// Shamelessly copy-pasted from <https://stackoverflow.com/a/40234666/5148606>.
macro_rules! fn_name {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        // We wouldn't want to crash if something goes wrong here (that would be
        // very confusing!).
        name.strip_suffix("::f")
            .unwrap_or("UNKNOWN")
            .split("::")
            .last()
            .unwrap_or("UNKNOWN")
    }};
}

#[proc_macro_derive(ModuleInfo, attributes(state, module, kernel_module, id, gas, phantom))]
pub fn module_info(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);

    handle_macro_error_and_expand(
        fn_name!(),
        module_info::derive_module_info(input, ModuleType::Standard),
    )
}

#[proc_macro_derive(
    KernelModuleInfo,
    attributes(state, module, kernel_module, id, gas, phantom)
)]
pub fn kernel_module_info(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);

    handle_macro_error_and_expand(
        fn_name!(),
        module_info::derive_module_info(input, ModuleType::Kernel),
    )
}

#[proc_macro_derive(Genesis)]
pub fn genesis(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    let genesis_macro = GenesisMacro::new("Genesis");

    handle_macro_error_and_expand(fn_name!(), genesis_macro.derive_genesis(input))
}

#[proc_macro_derive(DispatchCall, attributes(serialization))]
pub fn dispatch_call(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    let call_macro = DispatchCallMacro::new("Call");

    handle_macro_error_and_expand(fn_name!(), call_macro.derive_dispatch_call(input))
}

#[proc_macro_derive(Event, attributes(serialization))]
pub fn event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    let event_macro = EventMacro::new("Event");

    handle_macro_error_and_expand(fn_name!(), event_macro.derive_event_enum(input))
}

/// Adds encoding functionality to the underlying type.
#[proc_macro_derive(MessageCodec)]
pub fn codec(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    let codec_macro = MessageCodec::new("MessageCodec");

    handle_macro_error_and_expand(fn_name!(), codec_macro.derive_message_codec(input))
}

#[proc_macro]
pub fn config_bech32(tokens: TokenStream) -> TokenStream {
    struct ConstBech32Input {
        lit_str: syn::LitStr,
        ty: syn::Type,
    }

    impl syn::parse::Parse for ConstBech32Input {
        fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
            let lit_str = input.parse()?;
            input.parse::<syn::Token![,]>()?;
            let ty = input.parse()?;
            Ok(ConstBech32Input { lit_str, ty })
        }
    }

    let ConstBech32Input { lit_str, ty } = parse_macro_input!(tokens as ConstBech32Input);
    handle_macro_error_and_expand(fn_name!(), make_const_bech32(&lit_str, &ty))
}

#[proc_macro]
pub fn config_value(item: TokenStream) -> TokenStream {
    let constant_name = parse_macro_input!(item as syn::LitStr);
    handle_macro_error_and_expand(fn_name!(), make_const_value(&constant_name).map(Into::into))
}

/// A wrapper around [`jsonrpsee::proc_macros::rpc`] for modules.
///
/// This proc-macro generates a [`jsonrpsee`] implementation for the underlying
/// module type. It behaves very similar to the original [`jsonrpsee`]
/// proc-macro, but with some important distinctions:
///
/// 1. It's called `#[rpc_gen]` instead of `#[rpc]`, to avoid confusion with the
///    original proc-macro.
/// 2. `#[method]` is renamed to with `#[rpc_method]` to avoid confusion with
///    [`jsonrpsee`]'s own attribute.
/// 3. **It's applied on an `impl` block instead of a trait.** [`macro@rpc_gen`] will
///    copy all method definitions from your `impl` block into a new trait with
///    the same generics and method signatures. Unlike [`jsonrpsee`] traits
///    which can simply be signatures, these methods must all have function
///    bodies as they provide the trait definition and its implementation at the
///    same time.
/// 4. Working set arguments with signature `working_set: &mut WorkingSet<S>`
///    are automatically removed from the method signatures (as they are not
///    [`serde`]-compatible) and injected directly within the method bodies.
///
///    It sounds more complicated than it is. Generally, you can just assume
///    that the proc-macro will provide you with a working set argument that you
///    can request by adding it as a method argument.
///
/// Any code relying on this macro must take [`jsonrpsee`] as a dependency with
/// at least the following features enabled:
///
/// ```toml
/// jsonrpsee = { version = "...", features = ["macros", "client-core", "server"] }
/// ```
///
/// This proc-macro is only intended for modules. Refer to [`macro@expose_rpc`] for
/// the runtime proc-macro.
///
/// ## Example
/// ```
/// use sov_modules_api::{Spec, StateValue, ModuleId, ModuleInfo, WorkingSet};
/// use sov_modules_api::macros::rpc_gen;
/// use jsonrpsee::core::RpcResult;
///
/// #[derive(ModuleInfo)]
/// struct MyModule<S: Spec> {
///     #[id]
///     id: ModuleId,
///     #[state]
///     values: StateValue<S::Address>,
///     // ...
/// }
///
/// #[rpc_gen(client, server, namespace = "myNamespace")]
/// impl<S: Spec> MyModule<S> {
///     #[rpc_method(name = "myMethod")]
///     fn my_method(&self, working_set: &mut WorkingSet<S>, param: u32) -> RpcResult<S::Address> {
///         Ok(self.values.get(working_set).unwrap())
///     }
/// }
/// ```
#[proc_macro_attribute]
#[cfg(feature = "native")]
pub fn rpc_gen(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr_contents: Vec<syn::NestedMeta> = parse_macro_input!(attr);
    let input = parse_macro_input!(item as syn::ItemImpl);
    handle_macro_error_and_expand(
        fn_name!(),
        rpc::rpc_gen(attr_contents, input).map(|ok| ok.into()),
    )
}

#[cfg(feature = "native")]
#[proc_macro_attribute]
pub fn expose_rpc(_attr: TokenStream, input: TokenStream) -> TokenStream {
    let original = input.clone();
    let input = parse_macro_input!(input);
    handle_macro_error_and_expand(fn_name!(), rpc::expose_rpc("Expose", original, input))
}

#[cfg(feature = "native")]
#[proc_macro_derive(CliWallet, attributes(cli_skip))]
pub fn cli_parser(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    handle_macro_error_and_expand(fn_name!(), cli_parser::derive_cli_wallet("Cmd", input))
}

#[cfg(feature = "native")]
#[proc_macro_derive(CliWalletArg)]
pub fn custom_enum_clap(input: TokenStream) -> TokenStream {
    let input: syn::DeriveInput = parse_macro_input!(input);
    handle_macro_error_and_expand(fn_name!(), cli_parser::derive_cli_wallet_arg(input))
}

/// Simple convenience macro for adding some common derive macros and
/// impls specifically for a NewType wrapping an Address.
/// The reason for having this is that we assumes NewTypes for address as a common use case
///
/// ## Example
/// ```
///use sov_modules_macros::address_type;
///use std::fmt;
///use sov_modules_api::Spec;
///#[address_type]
///pub struct UserAddress;
/// ```
///
/// This is exactly equivalent to hand-writing
///
/// ```
/// use std::fmt;
/// use sov_modules_api::Spec;
///#[cfg(feature = "native")]
///#[derive(schemars::JsonSchema)]
///#[schemars(bound = "S::Address: ::schemars::JsonSchema", rename = "UserAddress")]
///#[derive(borsh::BorshDeserialize, borsh::BorshSerialize, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
///pub struct UserAddress<S: Spec>(S::Address);
///
///#[cfg(not(feature = "native"))]
///#[derive(borsh::BorshDeserialize, borsh::BorshSerialize, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
///pub struct UserAddress<S: Spec>(S::Address);
///
///impl<S: Spec> UserAddress<S> {
///    /// Public constructor
///    pub fn new(address: &S::Address) -> Self {
///        UserAddress(address.clone())
///    }
///
///    /// Public getter
///    pub fn get_address(&self) -> &S::Address {
///        &self.0
///    }
///}
///
///impl<S: Spec> fmt::Display for UserAddress<S>
///where
///    S::Address: fmt::Display,
///{
///    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
///        write!(f, "{}", self.0)
///    }
///}
///
///impl<S: Spec> AsRef<[u8]> for UserAddress<S>
///where
///    S::Address: AsRef<[u8]>,
///{
///    fn as_ref(&self) -> &[u8] {
///        self.0.as_ref()
///    }
///}
/// ```
#[proc_macro_attribute]
pub fn address_type(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    handle_macro_error_and_expand(fn_name!(), address_type_helper(input))
}

/// The offchain macro is used to annotate functions that should only be executed by the rollup
/// when the "offchain" feature flag is passed. The macro produces one of two functions depending on
/// the presence flag.
/// "offchain" feature enabled: function is present as defined
/// "offchain" feature absent: function body is replaced with an empty definition
///
/// The idea here is that offchain computation is optionally enabled for a full node and is not
/// part of chain state and does not impact consensus, prover or anything else.
///
/// ## Example
/// ```
/// use sov_modules_macros::offchain;
/// #[offchain]
/// fn redis_insert(count: u64){
///     println!("Inserting {} to redis", count);
/// }
/// ```
///
/// This is exactly equivalent to hand-writing
///```
/// #[cfg(feature = "offchain")]
/// fn redis_insert(count: u64){
///     println!("Inserting {} to redis", count);
/// }
///
/// #[cfg(not(feature = "offchain"))]
/// fn redis_insert(count: u64){
/// }
///```
#[proc_macro_attribute]
pub fn offchain(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    handle_macro_error_and_expand(fn_name!(), offchain_generator(input))
}

fn expand_code(macro_name: &str, input: TokenStream) -> TokenStream {
    if std::env::var_os("SOVEREIGN_SDK_EXPAND_PROC_MACROS").is_some() {
        expand_macro::expand_to_file(input.clone(), macro_name).unwrap_or_else(|err| {
            eprintln!(
                "Failed to write to file proc-macro generated code: {:?}",
                err
            );
            input
        })
    } else {
        input
    }
}

fn handle_macro_error_and_expand(
    macro_name: &str,
    result: Result<proc_macro::TokenStream, syn::Error>,
) -> TokenStream {
    expand_code(
        macro_name,
        result.unwrap_or_else(|err| err.to_compile_error().into()),
    )
}
