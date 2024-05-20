use darling::{ast, util, FromDeriveInput, FromField};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Generics, Ident};

use crate::common::{str_to_url_segment, wrap_in_new_scope};

pub fn derive(tokens: DeriveInput) -> syn::Result<TokenStream> {
    let input = InputStruct::from_derive_input(&tokens)?;

    // First, we ought to generate the code that will nest all module routers
    // into the root runtime router.
    let router_nest_ops = input
        .fields()
        .iter()
        // We happily ignore all fields marked with `skip`.
        .filter(|f| !f.skip)
        .map(|f| {
            let module_identifier = f.ident();
            let path = format!("/modules/{}", str_to_url_segment(module_identifier));

            quote! {
                {
                    let module_router: axum::Router<()> = (&self.#module_identifier).rest_api(storage.clone());
                    router = router.nest(#path, module_router);
                }
            }
        })
        .collect::<Vec<_>>();

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let ident = input.ident;

    let code = wrap_in_new_scope(quote! {
        use ::sov_modules_api::rest::*;
        use ::sov_modules_api::rest::__macros_private::*;
        use ::sov_modules_api::prelude::*;
        use ::sov_modules_api::hooks::TxHooks;

        #[automatically_derived]
        impl #impl_generics HasRestApi<<Self as TxHooks>::Spec> for #ident #ty_generics #where_clause {
            fn rest_api(&self, storage: StorageReceiver<<Self as TxHooks>::Spec>) -> axum::Router<()> {
                let base_impl = RuntimeRestApiBaseImpl {
                    // At the time of writing, runtimes are not guaranteed to be
                    // `Clone` but they are `Default`, so we create a new
                    // runtime instead of cloning `self`.
                    runtime: ::std::sync::Arc::new(Self::default()),
                };
                let mut router = base_impl.rest_api(storage.clone());

                #(#router_nest_ops)*

                let custom_router = HasCustomRestApi::<<Self as TxHooks>::Spec>::custom_rest_api(
                    &self, storage.clone()
                );
                router.merge(custom_router)
            }
        }
    });
    Ok(quote! {
        #[cfg(feature = "native")]
        #code
    })
}

#[derive(Debug, FromDeriveInput)]
#[darling(attributes(rest_api), supports(struct_named), forward_attrs(doc))]
pub struct InputStruct {
    pub ident: Ident,
    pub generics: Generics,
    pub data: ast::Data<util::Ignored, InputField>,
}

impl InputStruct {
    pub fn fields(&self) -> &[InputField] {
        match &self.data {
            ast::Data::Struct(s) => s.fields.as_slice(),
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, FromField)]
#[darling(attributes(rest_api), forward_attrs(doc))]
pub struct InputField {
    pub ident: Option<Ident>,
    #[darling(default)]
    pub skip: bool,
}

impl InputField {
    pub fn ident(&self) -> &Ident {
        self.ident.as_ref().unwrap_or_else(|| {
            panic!("darling invariant violated; struct is named so field must have an ident")
        })
    }
}
