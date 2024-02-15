use proc_macro2::Span;
use syn::DeriveInput;

use super::common::{get_generics_type_param, StructDef, StructFieldExtractor};

pub(crate) const EVENT: &str = "Event";

pub(crate) struct EventMacro {
    field_extractor: StructFieldExtractor,
}

impl<'a> StructDef<'a> {
    fn create_event_enum_legs(&self) -> Vec<proc_macro2::TokenStream> {
        self.fields
            .iter()
            .map(|field| {
                let name = &field.ident;
                let ty = &field.ty;

                quote::quote!(
                    #[doc = "Module event."]
                    #name(<#ty as ::sov_modules_api::Module>::Event),
                )
            })
            .collect()
    }
}

impl EventMacro {
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            field_extractor: StructFieldExtractor::new(name),
        }
    }

    pub(crate) fn derive_event_enum(
        &self,
        input: DeriveInput,
    ) -> Result<proc_macro::TokenStream, syn::Error> {
        let serialization_methods = vec![
            quote::quote! { borsh::BorshSerialize },
            quote::quote! { borsh::BorshDeserialize },
        ];

        let DeriveInput {
            data,
            ident,
            generics,
            ..
        } = input;

        let generic_param = get_generics_type_param(&generics, Span::call_site())?;
        let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
        let fields = self.field_extractor.get_fields_from_struct(&data)?;

        let struct_def = StructDef::new(
            ident,
            fields,
            impl_generics,
            type_generics,
            &generic_param,
            where_clause,
        );

        let event_enum_legs = struct_def.create_event_enum_legs();
        let event_enum = struct_def.create_enum(&event_enum_legs, EVENT, &serialization_methods);
        let event_cases = struct_def.fields.iter().map(|field| {
            let name = &field.ident;
            let module_ty = &field.ty;

            quote::quote! {
        _ if event.type_id() == &core::any::TypeId::of::<<#module_ty as ::sov_modules_api::Module>::Event>() => {
            event.downcast::<<#module_ty as ::sov_modules_api::Module>::Event>()
                  .map(|boxed_event| Self::RuntimeEvent::#name(boxed_event))
        }
    }
        });

        let impl_generics = &struct_def.impl_generics;
        let type_generics = &struct_def.type_generics;
        let ident_name = &struct_def.ident;
        let event_enum_name = struct_def.enum_ident(EVENT);

        let impl_runtime_event_processor = quote::quote! {
            impl #impl_generics ::sov_modules_api::RuntimeEventProcessor for #ident_name #type_generics {
                type RuntimeEvent = #event_enum_name #type_generics;

                fn convert_to_runtime_event<Co: ::sov_modules_api::Context>(
                    event: ::sov_modules_api::TypedEvent<Co>
                ) -> Option<Self::RuntimeEvent> {
                    match event.type_id() {
                        #(#event_cases)*
                        _ => None,
                    }
                }
            }
        };

        let impl_runtime_event_display = if cfg!(feature = "native") {
            quote::quote! {
                #[cfg(feature = "native")]
                impl #impl_generics ::sov_modules_api::RuntimeEventDisplay for #ident_name #type_generics #where_clause {
                    type RuntimeEvent = #event_enum_name #type_generics;
                }
            }
        } else {
            quote::quote! {}
        };

        let from_event_cases = struct_def.fields.iter().map(|field| {
            let variant_name = &field.ident;

            quote::quote! {
                #event_enum_name::#variant_name(ref event) => {
                    let event_data = serde_json::to_value(event).unwrap_or_default();
                    sov_rollup_interface::rpc::Event {
                        module_name: stringify!(#variant_name).to_string(),
                        event_value: event_data }
                }
            }
        });

        let impl_from = if cfg!(feature = "native") {
            quote::quote! {
                impl #impl_generics From<#event_enum_name #type_generics> for sov_rollup_interface::rpc::Event {
                    fn from(event: #event_enum_name #type_generics) -> Self {
                        match event {
                            #(#from_event_cases),*
                        }
                    }
                }
            }
        } else {
            quote::quote! {}
        };

        Ok(quote::quote! {
            #[doc="This enum is generated from the underlying Runtime, the variants correspond to events from the relevant modules"]
            #event_enum

            #impl_runtime_event_processor

            #impl_runtime_event_display

            #impl_from

        }
            .into())
    }
}
