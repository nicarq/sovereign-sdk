use proc_macro::TokenStream;
use quote::quote;
use syn::{Attribute, DeriveInput};

pub fn address_type_helper(input: DeriveInput) -> Result<TokenStream, syn::Error> {
    let name = &input.ident;
    let name_str = format!("{}", name);
    let attrs: Vec<Attribute> = input.attrs;

    let expanded = quote! {
        #[cfg(feature = "native")]
        #[derive(schemars::JsonSchema)]
        #[schemars(bound = "S::Address: ::schemars::JsonSchema", rename = #name_str)]
        #[derive(borsh::BorshDeserialize, borsh::BorshSerialize, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
        #(#attrs)*
        pub struct #name<S: ::sov_modules_api::Spec>(S::Address);

        #[cfg(not(feature = "native"))]
        #[derive(borsh::BorshDeserialize, borsh::BorshSerialize, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
        #(#attrs)*
        pub struct #name<S: ::sov_modules_api::Spec>(S::Address);

        impl<S: ::sov_modules_api::Spec> #name<S> {
            /// Public constructor
            pub fn new(address: &S::Address) -> Self {
                #name(address.clone())
            }

            /// Public getter
            pub fn get_address(&self) -> &S::Address {
                &self.0
            }
        }

        impl<S: ::sov_modules_api::Spec> fmt::Display for #name<S>
        where
            S::Address: fmt::Display,
        {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl<S: ::sov_modules_api::Spec> AsRef<[u8]> for #name<S>
        where
            S::Address: AsRef<[u8]>,
        {
            fn as_ref(&self) -> &[u8] {
                self.0.as_ref()
            }
        }
    };

    Ok(expanded.into())
}
