use darling::ast::{self, Data, Fields, Style};
use darling::usage::{CollectLifetimes, CollectTypeParams, GenericsExt, Purpose};
use darling::{uses_lifetimes, uses_type_params, FromDeriveInput, FromField, FromVariant};
use proc_macro2::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::token::{Comma, Where};
use syn::{DeriveInput, GenericParam, Generics, Ident, WhereClause, WherePredicate};

type SynResult<T> = Result<T, syn::Error>;

use darling::FromMeta;

use super::serde_rename::{parse_serde_rename_attrs, SerdeRename};

#[derive(Debug, FromMeta, Default, Clone)]
#[darling(rename_all = "snake_case")]
pub enum DisplayType {
    #[default]
    Hex,
    Decimal,
    Bech32 {
        #[darling(with = "darling::util::parse_expr::parse_str_literal")]
        prefix: syn::Expr,
    },
    Bech32m {
        #[darling(with = "darling::util::parse_expr::parse_str_literal")]
        prefix: syn::Expr,
    },
}

#[derive(Debug, FromMeta, Default, Clone)]
pub struct Bounds(syn::punctuated::Punctuated<syn::WherePredicate, Comma>);

impl ToTokens for Bounds {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.0.to_tokens(tokens)
    }
}

/// Derive the `SchemaGenerator` trait on an input type. See the actual proc-macro definition for
/// usage information, including attributes; the code generation is documented here.
///
/// The macro generates the `SchemaGenerator::scaffold()` and `SchemaGenerator::get_child_links()`
/// methods on the trait. The scaffold is the Container corresponding to the data structure of
/// the derived-on type, with the types of all child fields (if any) set as placeholder links.
/// `get_child_links()` is what allows schema generation to recursively walk every child type and
/// replace the placeholder links with a link to its corresponding schema.
///
/// For struct types, the generation is relatively self-explanatory: an `impl SchemaGenerator` is
/// emitted, containing the above two functions. However, enums are treated differently: any
/// variants that themselves contain fields are treated as containing a virtual struct. A struct
/// definition is then emitted, containing the actual data for that variant, with the proc macro
/// being in turn derived on it; the emitted code thus contains an impl for the enum type, structs
/// for every variant that contains fields, and corresponding impls for each of those structs.
pub fn derive(
    input: DeriveInput,
    prefix: Option<syn::TypePath>,
    macro_name: TokenStream,
) -> Result<TokenStream, syn::Error> {
    let res = derive_wallet_field(input, prefix, macro_name)?;
    if std::env::var("SOVEREIGN_SDK_EXPAND_PROC_MACROS").is_ok() {
        println!("------ Generated Wallet Output -------\n\n{}\n\n------ End Generated Wallet Output -------",&res);
    }
    Ok(res)
}

/// Collect the tokens to implement the return value of SchemaGenerator::get_child_links for a
/// struct type
fn struct_child_links(
    input: &Fields<InputField>,
    prefix: &Option<syn::TypePath>,
) -> Vec<TokenStream> {
    input
        .fields
        .iter()
        .filter(|field| !field.skip)
        .map(|item| maybe_generate_field_schema(item, prefix))
        .collect::<Vec<_>>()
}

/// Collect the tokens to implement one of the links returned as part of
/// SchemaGenerator::get_child_links for a single variant of an enum type
fn enum_variant_child_link(
    type_ident: &Ident,
    generics: &Generics,
    prefix: &Option<syn::TypePath>,
) -> TokenStream {
    let ty_generics = generics.split_for_impl().1;
    quote! {
        <#type_ident #ty_generics as #prefix::sov_universal_wallet::schema::SchemaGenerator>::make_linkable(schema)
    }
}

fn extend_where_clause_with_field_bounds(
    fields: &[InputField],
    where_clause: &mut Option<WhereClause>,
    prefix: &Option<syn::TypePath>,
) {
    let output = where_clause.get_or_insert(WhereClause {
        where_token: Where::default(),
        predicates: Default::default(),
    });
    add_self_bound_to_where_clause(output);
    for field in fields.iter().filter(|field| !field.skip) {
        let predicate = if let Some(bounds) = &field.bound {
            if bounds.0.is_empty() {
                continue;
            }
            syn::parse_quote! {
                #bounds
            }
        } else {
            let ty = field.ty_tokens();
            generate_where_clause_simple_field_bound(&ty, prefix)
        };
        output.predicates.push(predicate);
    }
    if output.predicates.is_empty() {
        *where_clause = None
    }
}

fn add_self_bound_to_where_clause(clause: &mut WhereClause) {
    clause.predicates.push(syn::parse_quote! { Self: 'static });
}

fn generate_where_clause_simple_field_bound(
    ty_tokens: &TokenStream,
    prefix: &Option<syn::TypePath>,
) -> WherePredicate {
    syn::parse_quote! {
        #ty_tokens: #prefix::sov_universal_wallet::schema::SchemaGenerator
    }
}

/// Main macro functionality. Implements the SchemaGenerator trait on the given type, by
/// a) generating a scaffold containing Link::Placeholder links for every child, and
/// b) collecting the types of every Placeholder child, in order, in make_child_links()
/// Schema generation will then traverse the child link types, generate the schema for those and
/// fill in the Placeholder links from the scaffold in order.
///
/// The only two types of data that can be an input to the macro are a struct or an enum. Struct
/// type handling is relatively self-explanatory. Enums, however, can contain data inside their
/// variants as "virtual" structs or tuples; we reify these into corresponding physical structs,
/// on which we recursively derive UniversalWallet. This allows the enum variants to be scaffolded
/// with Placeholders, which will get filled in with normal links during schema building.
/// (The alternative would've been to generate a nested scaffold in-place for the
/// entire type, for every variant in the enum, considerably bloating individual type).
fn derive_wallet_field(
    input: DeriveInput,
    prefix: Option<syn::TypePath>,
    macro_name: TokenStream,
) -> Result<TokenStream, syn::Error> {
    let DeriveInput {
        ident, generics, ..
    } = &input;
    let input = Input::from_derive_input(&input)?;
    let template_string = input.show_as;
    let template_tokens = quote_str_option_literally(&template_string);
    let serde_rename = input.attrs;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let mut where_clause = where_clause.cloned();
    let mut virtual_types: Vec<TokenStream> = vec![];
    let mut child_links: Vec<TokenStream> = vec![];
    let container = match &input.data {
        Data::Struct(s) => {
            child_links = struct_child_links(s, &prefix);
            match s.style {
                Style::Struct => build_struct_type_scaffold(
                    &s.fields,
                    input.ident.clone(),
                    serde_rename,
                    template_tokens,
                    &mut where_clause,
                    &prefix,
                )?,
                Style::Tuple => build_tuple_type_scaffold(
                    &s.fields,
                    template_tokens,
                    &mut where_clause,
                    &prefix,
                )?,
                Style::Unit => {
                    let serde_type_name = serde_rename.rename_typename(ident)?;
                    quote! {
                        #prefix::sov_universal_wallet::schema::Item::<#prefix::sov_universal_wallet::schema::IndexLinking>::Container(#prefix::sov_universal_wallet::schema::Container::Struct( #prefix::sov_universal_wallet::ty::Struct {
                            type_name: stringify!(#ident).to_string(),
                            serde_type_name: #serde_type_name.to_string(),
                            template: #template_tokens,
                            fields: vec![],
                        }))
                    }
                }
            }
        }
        Data::Enum(e) => {
            let where_under_construction = where_clause.get_or_insert(WhereClause {
                where_token: Where::default(),
                predicates: Default::default(),
            });
            let variants = e.iter()
            .map(|variant| {
                let variant_ident = &variant.ident;
                let virtual_type_generics = virtual_field_generics(generics.clone(), &variant.fields.fields);
                let virtual_type_ident = virtual_typename(ident, variant_ident);
                let variant_template = &variant.show_as;
                let variant_template_tokens = quote_str_option_literally(variant_template);
                let value = match &variant.fields.style {
                    Style::Struct => {
                        let virtual_struct = build_virtual_struct(&variant.fields.fields, where_under_construction, &virtual_type_ident, &virtual_type_generics, variant_template, &prefix, &macro_name)?;
                        virtual_types.push(virtual_struct);
                        child_links.push(enum_variant_child_link(&virtual_type_ident, &virtual_type_generics, &prefix));
                        quote!{ Some(#prefix::sov_universal_wallet::schema::Link::Placeholder) }
                    },
                    Style::Tuple => {
                        // TODO: Convert this to a warning and/or add an option to disable this check
                        if variant.fields.fields.len() > 1 && std::env::var("SOV_WALLET_PEDANTIC").is_ok() {
                            return Err(syn::Error::new_spanned(&input.ident, "Tuple structs with multiple entries are not human readable. Please use a named struct instead!"));
                        } else {
                            let virtual_tuple = build_virtual_tuple(&variant.fields.fields, where_under_construction, &virtual_type_ident, &virtual_type_generics, variant_template, &prefix, &macro_name)?;
                            virtual_types.push(virtual_tuple);
                            child_links.push(enum_variant_child_link(&virtual_type_ident, &virtual_type_generics, &prefix));
                            quote!{ Some(#prefix::sov_universal_wallet::schema::Link::Placeholder) }
                        }
                    },
                    Style::Unit => quote! { None },
                };
                let serde_variant_name = serde_rename.rename_variant(variant_ident)?;
                Ok::<TokenStream, syn::Error>(quote! {
                    #prefix::sov_universal_wallet::ty::EnumVariant {
                        name: stringify!(#variant_ident).to_string(),
                        serde_name: #serde_variant_name.to_string(),
                        template: #variant_template_tokens,
                        value: #value
                }})
            })
            .collect::<Result<Vec<_>, _>>()?;
            add_self_bound_to_where_clause(where_under_construction);

            let serde_type_name = serde_rename.rename_typename(ident)?;
            quote! {
                #prefix::sov_universal_wallet::schema::Item::<#prefix::sov_universal_wallet::schema::IndexLinking>::Container(#prefix::sov_universal_wallet::schema::Container::Enum(
                    #prefix::sov_universal_wallet::ty::Enum {
                        type_name: stringify!(#ident).to_string(),
                        serde_type_name: #serde_type_name.to_string(),
                        variants: vec![
                            #(#variants),*
                        ],
                    }
                ))
            }
        }
    };
    let child_links = quote! {
        vec! [
            #(#child_links),*
        ]
    };

    let schema = quote! {
        #[automatically_derived]
        impl #impl_generics #prefix::sov_universal_wallet::schema::SchemaGenerator for #ident #ty_generics #where_clause {
            fn scaffold() -> #prefix::sov_universal_wallet::schema::Item::<#prefix::sov_universal_wallet::schema::IndexLinking> {
                #container
            }

            fn get_child_links<M>(schema: &mut #prefix::sov_universal_wallet::schema::Schema<M>) -> Vec<#prefix::sov_universal_wallet::schema::Link> {
                #child_links
            }
        }

        #(#virtual_types) *
    };
    let res = schema.into_token_stream();
    Ok(res)
}

/// Take a struct type and return the appropriate scaffold for it. The scaffold is just
/// ```text
/// Item::Container(Container::Struct(Struct {
///   name: "MyStruct",
///   fields: vec![
///    NammedField {
///      name: "some_field_name",
///      value: Link::Placeholder,
///      # ...
///    },
///    # ...
/// ]
/// }))
/// ```
pub fn build_struct_type_scaffold(
    fields: &[InputField],
    type_name: Ident,
    serde_rename: SerdeRename,
    template_string: TokenStream,
    where_clause: &mut Option<WhereClause>,
    prefix: &Option<syn::TypePath>,
) -> Result<TokenStream, syn::Error> {
    extend_where_clause_with_field_bounds(fields, where_clause, prefix);

    let fields = fields
        .iter()
        .filter(|field| !field.skip)
        .map(|field| {
            let name = field
                .ident
                .as_ref()
                .expect("Struct types have named fields");
            let doc = String::new(); // TODO
            let silent = field.hidden;
            let serde_name = serde_rename.rename_field(name)?;
            SynResult::<_>::Ok(quote! {
                #prefix::sov_universal_wallet::ty::NamedField {
                    display_name: stringify!(#name).to_string(),
                    serde_display_name: #serde_name.to_string(),
                    value: #prefix::sov_universal_wallet::schema::Link::Placeholder,
                    silent: #silent,
                    doc: #doc.to_string(),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let serde_type_name = serde_rename.rename_typename(&type_name)?;
    Ok(quote! {
        #prefix::sov_universal_wallet::schema::Item::<#prefix::sov_universal_wallet::schema::IndexLinking>::Container(#prefix::sov_universal_wallet::schema::Container::Struct( #prefix::sov_universal_wallet::ty::Struct {
            type_name: stringify!(#type_name).to_string(),
            serde_type_name: #serde_type_name.to_string(),
            template: #template_string,
            fields: vec![#(#fields),*],
        }))
    })
}

/// Take a tuple type and return the appropriate scaffold for it. The scaffold is just
/// ```text
/// Item::Container(Container::Tuple(Tuple { fields: vec![
///    UnnamedField {
///      value: Link::Placeholder,
///      # ...
///    },
///    # ...
/// ]
/// }))
/// ```
pub fn build_tuple_type_scaffold(
    fields: &[InputField],
    template_string: TokenStream,
    where_clause: &mut Option<WhereClause>,
    prefix: &Option<syn::TypePath>,
) -> Result<TokenStream, syn::Error> {
    extend_where_clause_with_field_bounds(fields, where_clause, prefix);

    let fields = fields
        .iter()
        .filter(|field| !field.skip)
        .collect::<Vec<_>>();
    // This might be the root cause of the bug we were seeing with HexHash
    // // Don't add an extra virtual tuple if the definition only has a single field.
    // // Instead, just return the field schema directly.
    // if fields.len() == 1 && matches!(skew, MaybeVirtualType::Virtual { .. }) {
    //     let resolved_value = fields[0].resolve_type(prefix);
    //     return Ok(quote! {
    //         #resolved_value
    //     });
    // }

    let fields = fields
        .iter()
        .filter(|field| !field.skip)
        .map(|field| {
            let doc = String::new(); // TODO
            let silent = field.hidden;
            SynResult::<_>::Ok(quote! {
                #prefix::sov_universal_wallet::ty::UnnamedField {
                    value: #prefix::sov_universal_wallet::schema::Link::Placeholder,
                    silent: #silent,
                    doc: #doc.to_string(),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(quote! {
        #prefix::sov_universal_wallet::schema::Item::<#prefix::sov_universal_wallet::schema::IndexLinking>::Container(#prefix::sov_universal_wallet::schema::Container::Tuple( #prefix::sov_universal_wallet::ty::Tuple {
            template: #template_string,
            fields: vec![#(#fields),*],
        }))
    })
}

/// The name format for the generated types representing the contents of enum variants. Not meant
/// to be explicitly interacted with in user code or displayed to the user.
fn virtual_typename(enum_ident: &Ident, variant_ident: &Ident) -> Ident {
    format_ident!("__SovVirtualWallet_{}_{}", enum_ident, variant_ident)
}

/// When generating over an enum, filter the parent field's generics to those used in each variant.
/// Here `fields` are the subfields of the enum variant.
/// This is used to generate the virtual struct representing the variant's data, with the correct
/// generic bounds for the field.
fn virtual_field_generics(input_generics: Generics, fields: &[InputField]) -> Generics {
    let input_params = input_generics.declared_type_params();
    let input_lifetimes = input_generics.declared_lifetimes();
    let collected_generics = fields.collect_type_params(&Purpose::Declare.into(), &input_params);
    let collected_lifetimes = fields.collect_lifetimes(&Purpose::Declare.into(), &input_lifetimes);
    let mut virt_generics = input_generics.clone();
    virt_generics.params = virt_generics
        .params
        .into_iter()
        .filter(|gp| match gp {
            GenericParam::Type(t) => collected_generics.contains(&t.ident),
            GenericParam::Lifetime(l) => collected_lifetimes.contains(&l.lifetime),
            GenericParam::Const(_) => true,
        })
        .collect();
    virt_generics
}

/// Build an explicit struct type from the fields inside an enum variant.
/// This is a relatively simple wrapper around the variant's fields, the only extra complexity lies
/// in generating the correct bounds for the union of the generics used by that variant's fields.
///
/// Crucially, we then recursively #[derive(UniversalWallet)] on it.
/// This has an important consequence for hygiene, as the crate does not define the macro, so we
/// require that `UniversalWallet` be defined and in scope.
fn build_virtual_struct(
    fields: &[InputField],
    where_clause: &mut WhereClause,
    type_name: &Ident,
    type_generics: &Generics,
    template_string: &Option<String>,
    prefix: &Option<syn::TypePath>,
    macro_name: &TokenStream,
) -> Result<TokenStream, syn::Error> {
    let (virt_impl_generics, virt_ty_generics, virt_where_clause) = type_generics.split_for_impl();
    where_clause
        .predicates
        .push(generate_where_clause_simple_field_bound(
            &quote! { #type_name #virt_ty_generics },
            prefix,
        ));
    let struct_fields: Vec<_> = fields
        .iter()
        .map(|field| {
            let name = &field.ident;
            let field_type = &field.ty;
            let field_bounds = field.bound.as_ref().map(|b| {
                let tokens = format!("{}", b.to_token_stream());
                quote! {
                    #[sov_wallet(bound=#tokens)]
                }
            });
            // TODO: propagate `#[serde(rename)]` attributes to each field here if support for it
            // is required
            quote! {
                #field_bounds #name: #field_type
            }
        })
        .collect();

    // TODO: pass attribute name as argument when creating the derive, instead of hardcoding
    // sov_wallet
    let template_attribute = match template_string {
        Some(template) => quote! {#[sov_wallet(show_as = #template)]},
        None => quote! {},
    };

    // TODO: propagate `#[serde(rename_all_fields)]` here, transformed into a `rename_all`,
    // if support for that is required
    let virtual_struct = quote! {
        #[allow(non_camel_case_types, dead_code)]
        #[automatically_derived]
        #[derive(#macro_name)]
        #template_attribute
        struct #type_name #virt_impl_generics #virt_where_clause {
            #(#struct_fields),*
        }
    };

    Ok(virtual_struct)
}

/// Build an explicit tuple struct type from the fields inside an enum variant.
/// This is a relatively simple wrapper around the variant's fields, the only extra complexity lies
/// in generating the correct bounds for the union of the generics used by that variant's fields.
///
/// Crucially, we then recursively #[derive(UniversalWallet)] on it.
/// This has an important consequence for hygiene, as the crate does not define the macro, so we
/// require that `UniversalWallet` be defined and in scope.
fn build_virtual_tuple(
    fields: &[InputField],
    where_clause: &mut WhereClause,
    type_name: &Ident,
    type_generics: &Generics,
    template_string: &Option<String>,
    prefix: &Option<syn::TypePath>,
    macro_name: &TokenStream,
) -> Result<TokenStream, syn::Error> {
    let (virt_impl_generics, virt_ty_generics, virt_where_clause) = type_generics.split_for_impl();
    where_clause
        .predicates
        .push(generate_where_clause_simple_field_bound(
            &quote! { #type_name #virt_ty_generics },
            prefix,
        ));
    let tuple_fields: Vec<_> = fields
        .iter()
        .map(|field| {
            let field_type = &field.ty;
            let field_bounds = field.bound.as_ref().map(|b| {
                let tokens = format!("{}", b.to_token_stream());
                quote! {
                    #[sov_wallet(bound=#tokens)]
                }
            });
            quote! {
                #field_bounds #field_type
            }
        })
        .collect();

    // TODO: pass attribute name as argument when creating the derive, instead of hardcoding
    // sov_wallet
    let template_attribute = match template_string {
        Some(template) => quote! {#[sov_wallet(show_as = #template)]},
        None => quote! {},
    };

    // TODOs as above for build_virtual_struct
    let virtual_tuple = quote! {
        #[allow(non_camel_case_types, dead_code)]
        #[automatically_derived]
        #[derive(#macro_name)]
        #template_attribute
        struct #type_name #virt_impl_generics #virt_where_clause (
            #(#tuple_fields),*
        );
    };

    Ok(virtual_tuple)
}

fn maybe_generate_field_schema(field: &InputField, prefix: &Option<syn::TypePath>) -> TokenStream {
    field.resolve_type(prefix)
}

/// necessary to have quote! actually include the Option tokens
/// String options get quoted as a static str, so instead of a generic util this is a simple
/// special-cased helper due to the need to call .to_string()
fn quote_str_option_literally(opt: &Option<String>) -> TokenStream {
    match opt {
        Some(s) => quote! { Some(#s.to_string()) },
        None => quote! { None },
    }
}

#[derive(Debug, FromDeriveInput)]
#[darling(attributes(sov_wallet), supports(any), forward_attrs(doc, serde))]
pub struct Input {
    pub ident: Ident,
    pub data: ast::Data<InputVariant, InputField>,
    #[darling(with = "parse_serde_rename_attrs")]
    pub attrs: SerdeRename,
    #[darling(default)]
    pub show_as: Option<String>,
}

#[derive(Debug, Clone, FromField)]
#[darling(attributes(sov_wallet), forward_attrs(doc, serde))]
pub struct InputField {
    pub attrs: Vec<syn::Attribute>,
    pub ident: Option<Ident>,
    pub ty: syn::Type,
    #[darling(default)]
    pub skip: bool,
    #[darling(default)]
    pub display: Option<DisplayType>,
    #[darling(default)]
    pub bound: Option<Bounds>,
    #[darling(default)]
    pub hidden: bool,
    #[darling(default, rename = "as_ty")]
    pub as_ty: Option<syn::Type>,
}

uses_type_params!(InputField, ty);
uses_lifetimes!(InputField, ty);

impl InputField {
    pub fn ty_tokens(&self) -> TokenStream {
        if let Some(ty) = &self.as_ty {
            quote! { #ty }
        } else {
            let ty = &self.ty;
            quote! { #ty }
        }
    }

    pub fn resolve_type(&self, crate_prefix: &Option<syn::TypePath>) -> TokenStream {
        let ty = self.ty_tokens();
        if let Some(display) = &self.display {
            let display_tokens = match display {
                DisplayType::Hex => {
                    quote! { #crate_prefix::sov_universal_wallet::ty::ByteDisplay::Hex }
                }
                DisplayType::Decimal => {
                    quote! { #crate_prefix::sov_universal_wallet::ty::ByteDisplay::Decimal }
                }
                DisplayType::Bech32 { prefix } => {
                    quote! { #crate_prefix::sov_universal_wallet::ty::ByteDisplay::Bech32 { prefix: #crate_prefix::sov_universal_wallet::bech32::Hrp::parse(#prefix).expect("Invalid bech32 prefix") } }
                }
                DisplayType::Bech32m { prefix } => {
                    quote! { #crate_prefix::sov_universal_wallet::ty::ByteDisplay::Bech32m { prefix: #crate_prefix::sov_universal_wallet::bech32::Hrp::parse(#prefix).expect("Invalid bech32 prefix") } }
                }
            };
            quote! {
                {
                    // #crate_prefix::sov_universal_wallet::ty::ByteDisplayable;
                    <#ty as #crate_prefix::sov_universal_wallet::ty::ByteDisplayable>::with_display(#display_tokens)
                }
            }
        } else {
            quote! {
                <#ty as #crate_prefix::sov_universal_wallet::schema::SchemaGenerator>::make_linkable(schema)
            }
        }
    }
}

#[derive(Debug, Clone, FromVariant)]
#[darling(attributes(sov_wallet), forward_attrs(doc))]
pub struct InputVariant {
    pub ident: Ident,
    pub fields: ast::Fields<InputField>,
    #[darling(default)]
    pub show_as: Option<String>,
}
