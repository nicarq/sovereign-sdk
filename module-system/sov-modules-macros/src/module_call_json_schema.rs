use syn::DeriveInput;

pub fn derive_module_call_json_schema(input: DeriveInput) -> syn::Result<proc_macro::TokenStream> {
    let DeriveInput {
        ident, generics, ..
    } = input;

    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    let tokens = quote::quote! {
        #[automatically_derived]
        impl #impl_generics ::sov_modules_api::ModuleCallJsonSchema for #ident #type_generics #where_clause {
            fn json_schema() -> ::std::string::String {
                use ::schemars::JsonSchema as _;

                let schema = ::schemars::schema_for!(
                    <Self as ::sov_modules_api::Module>::CallMessage
                );
                ::serde_json::to_string_pretty(&schema).expect("Failed to serialize JSON schema; this is a bug in the module")
            }
        }
    };

    Ok(tokens.into())
}
