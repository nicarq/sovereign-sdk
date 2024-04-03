use std::str::FromStr;

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::{Attribute, FnArg, ImplItem, Meta, MetaList, PatType, Path, Signature};

/// Returns an attribute with the name `rpc_method` replaced with `method`, and the index
/// into the argument array where the attribute was found.
fn get_method_attribute(attributes: &[Attribute]) -> Option<(Attribute, usize)> {
    for (idx, attribute) in attributes.iter().enumerate() {
        if let Ok(Meta::List(MetaList { path, .. })) = attribute.parse_meta() {
            if path.is_ident("rpc_method") {
                let mut new_attr = attribute.clone();
                let path = &mut new_attr.path;
                path.segments.last_mut().unwrap().ident = format_ident!("method");
                return Some((new_attr, idx));
            }
        }
    }
    None
}

fn jsonrpsee_rpc_macro_path() -> Path {
    syn::parse_quote! { ::jsonrpsee::proc_macros::rpc }
}

fn find_working_set_argument(sig: &Signature) -> syn::Result<Option<(usize, syn::Type)>> {
    let error = || {
        syn::Error::new_spanned(
        sig,
        "The `working_set` argument to the `#[rpc_method]`-annotated function has the wrong type. It should be a mutable reference to a `WorkingSet<...>`. Either fix the type or remove the `working_set` argument.",
    )
    };

    for (idx, input) in sig.inputs.iter().enumerate() {
        // We're not interested in `self`.
        let FnArg::Typed(PatType { ty, pat, .. }) = input else {
            continue;
        };

        // We're only interested in arguments that bind a new variable.
        let syn::Pat::Ident(syn::PatIdent { ident, .. }) = *pat.clone() else {
            continue;
        };

        if ident != "working_set" && ident != "_working_set" {
            continue;
        }

        // It should be a reference...
        let syn::Type::Reference(syn::TypeReference {
            elem, mutability, ..
        }) = *ty.clone()
        else {
            return Err(error());
        };

        // ...to a `syn::Type`!
        let syn::Type::Path(syn::TypePath { path, .. }) = elem.as_ref() else {
            return Err(error());
        };

        // It must be a *mutable* reference!
        if mutability.is_none() {
            return Err(error());
        }

        // Let's inspect the type path.
        let Some(segment) = path.segments.last() else {
            return Err(error());
        };

        // It must have generic arguments.
        let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
            return Err(error());
        };

        // It must have exactly *one* generic argument.
        if args.args.len() != 1 {
            return Err(error());
        }

        // Finally.
        return Ok(Some((idx, *elem.clone())));
    }
    Ok(None)
}

struct RpcEnabledMethod {
    pub method: syn::ImplItemMethod,
    pub docs: Vec<Attribute>,
    pub rpc_attribute: (Attribute, usize),
    pub working_set_arg: Option<(usize, syn::Type)>,
}

impl RpcEnabledMethod {
    fn parse(method: &syn::ImplItemMethod) -> Result<Option<Self>, syn::Error> {
        let Some(rpc_attribute) = get_method_attribute(&method.attrs) else {
            return Ok(None);
        };

        let working_set_arg = find_working_set_argument(&method.sig)?;
        let docs = method
            .attrs
            .iter()
            .filter(|attr| attr.path.is_ident("doc"))
            .cloned()
            .collect::<Vec<_>>();

        Ok(Some(Self {
            method: method.clone(),
            rpc_attribute,
            docs,
            working_set_arg,
        }))
    }

    /// Builds the annotated signature for the intermediate trait.
    fn annotated_signature_for_intermediate_trait(&self) -> TokenStream {
        let mut intermediate_trait_inputs = self.method.sig.inputs.clone();
        if let Some((idx, _)) = self.working_set_arg {
            // Remove the working set argument from the intermediate trait signature
            let mut inputs: Vec<syn::FnArg> = intermediate_trait_inputs.into_iter().collect();
            inputs.remove(idx);
            intermediate_trait_inputs = inputs.into_iter().collect();
        }

        let mut intermediate_signature = self.method.sig.clone();
        // Remove the working set argument from the signature
        intermediate_signature.inputs = intermediate_trait_inputs;

        let docs = &self.docs;
        let rpc_attribute = &self.rpc_attribute.0;

        quote! {
            #( #docs )*
            #rpc_attribute
            #intermediate_signature;
        }
    }

    /// Returns an identical copy of hte original method, but with the `#[method_rpc]`
    /// attribute removed.
    fn method_without_rpc_attr(&self) -> syn::ImplItemMethod {
        let mut method = self.method.clone();
        method.attrs.remove(self.rpc_attribute.1);
        method
    }

    fn name(&self) -> &Ident {
        &self.method.sig.ident
    }

    fn signature(&self) -> &Signature {
        &self.method.sig
    }

    // Returns the names of the method' arguments.
    fn arg_names(&self) -> impl Iterator<Item = TokenStream> + Clone + '_ {
        self.signature().inputs.iter().map(|item| {
            if let FnArg::Typed(PatType { pat, .. }) = item {
                if let syn::Pat::Ident(syn::PatIdent { ref ident, .. }) = &**pat {
                    return quote! { #ident };
                }
                unreachable!("Expected a pattern identifier")
            } else {
                quote! { self }
            }
        })
    }
}

struct RpcImplBlock {
    pub type_name: Ident,
    pub methods: Vec<RpcEnabledMethod>,
    pub working_set_type: Option<syn::Type>,
    pub generics: syn::Generics,
}

impl RpcImplBlock {
    fn impl_trait_name(&self) -> Ident {
        format_ident!("{}RpcImpl", self.type_name)
    }

    /// Builds the trait `_RpcImpl` That will be implemented by the runtime
    fn build_rpc_impl_trait(&self) -> TokenStream {
        let generics = &self.generics;
        let where_clause = generics.split_for_impl().2;

        let impl_trait_name = self.impl_trait_name();

        let (impl_trait_methods, blanket_impl_methods): (Vec<TokenStream>, Vec<TokenStream>) = self
            .methods
            .iter()
            .map(|method| self.impl_trait_and_blanket_method(method))
            .unzip();

        let rpc_impl_trait = {
            let get_working_set_method_opt = self
                .working_set_type
                .as_ref()
                .map(|ws_type| {
                    quote! {
                        /// Gets a clean working set on top of the latest state.
                        fn get_working_set(&self) -> #ws_type;
                    }
                })
                // No method at all if there is no working set type.
                .unwrap_or_default();

            quote! {
                /// Allows a `Runtime` to be converted into a functional RPC
                /// server by simply implementing a handful of methods.
                pub trait #impl_trait_name #generics #where_clause {
                    #get_working_set_method_opt
                    #(#impl_trait_methods)*
                }
            }
        };

        let blanket_impl = self.build_blanket_impl(blanket_impl_methods);

        quote! {
            #rpc_impl_trait
            #blanket_impl
        }
    }

    fn build_blanket_impl(&self, methods: Vec<TokenStream>) -> TokenStream {
        let generics = &self.generics;
        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let impl_trait_name = self.impl_trait_name();

        let blanket_impl_generics = quote! {
            #impl_generics
        }
        .to_string();
        let blanket_impl_generics_without_braces = proc_macro2::TokenStream::from_str(
            &blanket_impl_generics[1..blanket_impl_generics.len() - 1],
        )
        .expect("Failed to parse generics without braces as token stream");

        let rpc_server_trait_name = format_ident!("{}RpcServer", self.type_name);
        let blanket_impl = quote! {
            impl <
                MacroGeneratedTypeWithLongNameToAvoidCollisions:
                    #impl_trait_name #ty_generics + ::core::marker::Send + ::core::marker::Sync + 'static,
                #blanket_impl_generics_without_braces
            > #rpc_server_trait_name #ty_generics for MacroGeneratedTypeWithLongNameToAvoidCollisions #where_clause {
                #(#methods)*
            }
        };

        quote! {
            #blanket_impl
        }
    }

    fn impl_trait_and_blanket_method(
        &self,
        method: &RpcEnabledMethod,
    ) -> (TokenStream, TokenStream) {
        let impl_trait_name = self.impl_trait_name();
        let type_name = &self.type_name;
        let generics = &self.generics;
        let ty_generics = generics.split_for_impl().1;

        let method_name = &method.name();
        let docs = &method.docs;
        let mut signature = method.signature().clone();
        let arg_names = method.arg_names();

        let impl_trait_method = if let Some((idx, _)) = method.working_set_arg {
            // If necessary, adjust the signature to remove the working set argument and replace it with one generated by the implementer.
            // Remove the "self" argument as well
            let pre_working_set_args = arg_names
                .clone()
                .take(idx)
                .filter(|arg| arg.to_string() != quote! { self }.to_string());
            let post_working_set_args = arg_names
                .clone()
                .skip(idx + 1)
                .filter(|arg| arg.to_string() != quote! { self }.to_string());
            let mut inputs: Vec<syn::FnArg> = signature.inputs.clone().into_iter().collect();
            inputs.remove(idx);

            signature.inputs = inputs.into_iter().collect();

            quote! {
                #( #docs )*
                #signature {
                    <#type_name #ty_generics as ::std::default::Default>::default().#method_name(#(#pre_working_set_args,)* &mut Self::get_working_set(self), #(#post_working_set_args),* )
                }
            }
        } else {
            // Remove the "self" argument, since the method is invoked on `self` using dot notation
            let arg_values = arg_names
                .clone()
                .filter(|arg| arg.to_string() != quote! { self }.to_string());
            quote! {
                #( #docs )*
                #signature {
                    let default_module = <#type_name #ty_generics as ::std::default::Default>::default();
                    default_module.#method_name(#(#arg_values),*)
                }
            }
        };

        let blanket_impl_method = if let Some((idx, _)) = method.working_set_arg {
            // If necessary, adjust the signature to remove the working set argument.
            let pre_working_set_args = arg_names.clone().take(idx);
            let post_working_set_args = arg_names.clone().skip(idx + 1);
            quote! {
                #( #docs )*
                #signature {
                    <Self as #impl_trait_name #ty_generics >::#method_name(#(#pre_working_set_args,)* #(#post_working_set_args),* )
                }
            }
        } else {
            quote! {
                #( #docs )*
                #signature {
                    <Self as #impl_trait_name #ty_generics >::#method_name(#(#arg_names),*)
                }
            }
        };

        (impl_trait_method, blanket_impl_method)
    }

    /// If the working set type is not set, set it.
    /// If it is, we need to check that it's the same type.
    fn set_working_set_type(&mut self, method: &RpcEnabledMethod) -> syn::Result<()> {
        let method_ws_type = method.working_set_arg.as_ref().map(|arg| arg.1.clone());
        match (&self.working_set_type, &method_ws_type) {
            (Some(ws), Some(ref method_ws_type)) if ws != method_ws_type => {
                return Err(syn::Error::new_spanned(
                    method.name(),
                    format!("All `#[rpc_method]` annotated methods must have the same working set type. Found `{:?}` and `{:?}`", ws, method_ws_type),
                ));
            }
            // The method has no working set argument; do nothing.
            (_, None) => {}
            _ => self.working_set_type = method_ws_type,
        };

        Ok(())
    }
}

fn add_server_bounds_attr_if_missing(attrs: &mut Vec<syn::NestedMeta>) {
    for attr in attrs.iter() {
        if let syn::NestedMeta::Meta(syn::Meta::List(syn::MetaList { path, .. })) = attr {
            if path.is_ident("server_bounds") {
                return;
            }
        }
    }
    attrs.push(syn::NestedMeta::Meta(syn::Meta::List(
        syn::parse_quote! { server_bounds() },
    )));
}

fn add_client_bounds_attr_if_missing(attrs: &mut Vec<syn::NestedMeta>) {
    for attr in attrs.iter() {
        if let syn::NestedMeta::Meta(syn::Meta::List(syn::MetaList { path, .. })) = attr {
            if path.is_ident("client_bounds") {
                return;
            }
        }
    }
    attrs.push(syn::NestedMeta::Meta(syn::Meta::List(
        syn::parse_quote! { client_bounds() },
    )));
}

fn build_rpc_trait(
    mut attrs: Vec<syn::NestedMeta>,
    type_name: Ident,
    mut input: syn::ItemImpl,
) -> syn::Result<TokenStream> {
    let intermediate_trait_name = format_ident!("{}Rpc", type_name);
    // If the user hasn't directly provided trait bounds, override jsonrpsee's defaults
    // with an empty bound. This prevents spurious compilation errors like `Spec does not implement DeserializeOwned`
    add_server_bounds_attr_if_missing(&mut attrs);
    add_client_bounds_attr_if_missing(&mut attrs);

    let wrapped_attr_args = quote! {
        ( #(#attrs),* )
    };
    let rpc_attribute = syn::Attribute {
        pound_token: syn::token::Pound {
            spans: [proc_macro2::Span::call_site()],
        },
        style: syn::AttrStyle::Outer,
        bracket_token: syn::token::Bracket {
            span: proc_macro2::Span::call_site(),
        },
        path: jsonrpsee_rpc_macro_path(),
        tokens: wrapped_attr_args,
    };
    // Iterate over the methods from the `impl` block, building up three lists of items as we go

    let generics = &input.generics;
    let mut rpc_info = RpcImplBlock {
        type_name: type_name.clone(),
        methods: vec![],
        working_set_type: None,
        generics: generics.clone(),
    };

    let mut intermediate_trait_items = vec![];
    let mut simplified_impl_items = vec![];
    for item in input.items.into_iter() {
        if let ImplItem::Method(ref method) = item {
            if let Some(method) = RpcEnabledMethod::parse(method)? {
                rpc_info.set_working_set_type(&method)?;
                intermediate_trait_items.push(method.annotated_signature_for_intermediate_trait());
                simplified_impl_items.push(ImplItem::Method(method.method_without_rpc_attr()));

                rpc_info.methods.push(method);
                continue;
            }
        }
        simplified_impl_items.push(item);
    }

    let impl_rpc_trait_impl = rpc_info.build_rpc_impl_trait();

    // Replace the original impl block with a new version with the rpc_gen and related annotations removed
    input.items = simplified_impl_items;
    let simplified_impl = quote! {
        #input
    };

    let doc_string = format!("Generated RPC trait for {}", type_name);
    let (_, ty_generics, where_clause) = generics.split_for_impl();

    let rpc_output = quote! {
        #simplified_impl

        #impl_rpc_trait_impl


        #rpc_attribute
        #[doc = #doc_string]
        pub trait #intermediate_trait_name  #generics #where_clause {

            #(#intermediate_trait_items)*

            /// Check the health of the RPC server
            #[method(name = "health")]
            fn health(&self) -> ::jsonrpsee::core::RpcResult<()> {
                Ok(())
            }

            /// Get the ID of this module
            #[method(name = "moduleId")]
            fn module_id(&self) -> ::jsonrpsee::core::RpcResult<String> {
                Ok(<#type_name #ty_generics as ::sov_modules_api::ModuleInfo>::id(&<#type_name #ty_generics as ::core::default::Default>::default()).to_string())
            }

        }
    };
    Ok(rpc_output)
}

pub fn rpc_gen(
    attrs: Vec<syn::NestedMeta>,
    input: syn::ItemImpl,
) -> Result<proc_macro2::TokenStream, syn::Error> {
    let type_name = match *input.self_ty {
        syn::Type::Path(ref type_path) => &type_path.path.segments.last().unwrap().ident,
        _ => return Err(syn::Error::new_spanned(input.self_ty, "Invalid type")),
    };

    build_rpc_trait(attrs, type_name.clone(), input)
}
