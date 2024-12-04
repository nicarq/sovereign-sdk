use proc_macro2::{Span, TokenStream};
use syn::{DeriveInput, Ident};

use crate::common::{StructFieldExtractor, StructNamedField};

pub(crate) struct HooksMacro {
    field_extractor: StructFieldExtractor,
}

struct ArgWithType {
    arg: Ident,
    ty: TokenStream,
}

impl HooksMacro {
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            field_extractor: StructFieldExtractor::new(name),
        }
    }

    /// Derives the [`::sov_modules_api::SlotHooks`], [`::sov_modules_api::TxHooks`] and [`::sov_modules_api::FinalizeHook`] traits.
    pub(crate) fn derive_hooks(&self, input: DeriveInput) -> syn::Result<proc_macro::TokenStream> {
        let DeriveInput {
            data,
            ident,
            generics,
            ..
        } = input;

        let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

        let fields = self.field_extractor.get_fields_from_struct(&data)?;

        let slot_hooks_impl = Self::derive_slot_hooks(
            &ident,
            &fields,
            &impl_generics,
            &type_generics,
            &where_clause,
        );

        let kernel_slot_hooks_impl = Self::derive_kernel_slot_hooks(
            &ident,
            &fields,
            &impl_generics,
            &type_generics,
            &where_clause,
        );

        let finalize_hook_impl = Self::derive_finalize_hook(
            &ident,
            &fields,
            &impl_generics,
            &type_generics,
            &where_clause,
        );

        let tx_hooks_impl = Self::derive_tx_hooks(
            &ident,
            &fields,
            &impl_generics,
            &type_generics,
            &where_clause,
        );

        Ok(quote::quote! {
            mod slot_hooks {
                use super::*;
                #slot_hooks_impl
            }

            mod kernel_slot_hooks {
                use super::*;
                #kernel_slot_hooks_impl
            }

            mod finalize_hook {
                use super::*;
                ::sov_modules_api::native_only!(#finalize_hook_impl);
            }

            mod tx_hooks {
                use super::*;
                #tx_hooks_impl
            }
        }
        .into())
    }

    fn derive_slot_hooks(
        ident: &Ident,
        fields: &[StructNamedField],
        impl_generics: &syn::ImplGenerics,
        type_generics: &syn::TypeGenerics,
        where_clause: &Option<&syn::WhereClause>,
    ) -> proc_macro2::TokenStream {
        let begin_slot_hook_fn = Self::make_hooks_fn(
            fields,
            &Ident::new("begin_slot_hook", Span::call_site()),
            &vec![],
            false,
            vec![
                &ArgWithType {
                    arg: Ident::new("visible_hash", Span::call_site()),
                    ty: quote::quote! {&<<Self::Spec as ::sov_modules_api::Spec>::Storage as ::sov_modules_api::Storage>::Root},
                },
                &ArgWithType {
                    arg: Ident::new("state", Span::call_site()),
                    ty: quote::quote! {&mut ::sov_modules_api::StateCheckpoint<<Self::Spec as ::sov_modules_api::Spec>::Storage>},
                },
            ],
        );
        let end_slot_hook_fn = Self::make_hooks_fn(
            fields,
            &Ident::new("end_slot_hook", Span::call_site()),
            &vec![],
            false,
            vec![&ArgWithType {
                arg: Ident::new("state", Span::call_site()),
                ty: quote::quote! {&mut sov_modules_api::StateCheckpoint<<Self::Spec as ::sov_modules_api::Spec>::Storage>},
            }],
        );

        quote::quote! {
            use ::sov_modules_api::hooks::SlotHooks;

            impl #impl_generics ::sov_modules_api::SlotHooks for #ident #type_generics #where_clause {
                type Spec = <Self as ::sov_modules_api::DispatchCall>::Spec;

                #begin_slot_hook_fn

                #end_slot_hook_fn
            }
        }
    }

    fn derive_kernel_slot_hooks(
        ident: &Ident,
        fields: &[StructNamedField],
        impl_generics: &syn::ImplGenerics,
        type_generics: &syn::TypeGenerics,
        where_clause: &Option<&syn::WhereClause>,
    ) -> proc_macro2::TokenStream {
        let begin_slot_hook_fn = Self::make_hooks_fn(
            fields,
            &Ident::new("kernel_begin_slot_hook", Span::call_site()),
            &vec![],
            false,
            vec![
                &ArgWithType {
                    arg: Ident::new("slot_header", Span::call_site()),
                    ty: quote::quote! {&<<Self::Spec as ::sov_modules_api::Spec>::Da as ::sov_modules_api::DaSpec>::BlockHeader},
                },
                &ArgWithType {
                    arg: Ident::new("validity_condition", Span::call_site()),
                    ty: quote::quote! {&<<Self::Spec as ::sov_modules_api::Spec>::Da as ::sov_modules_api::DaSpec>::ValidityCondition},
                },
                &ArgWithType {
                    arg: Ident::new("pre_state_root", Span::call_site()),
                    ty: quote::quote! {&<<Self::Spec as ::sov_modules_api::Spec>::Storage as ::sov_modules_api::Storage>::Root},
                },
                &ArgWithType {
                    arg: Ident::new("state", Span::call_site()),
                    ty: quote::quote! {&mut ::sov_modules_api::KernelStateAccessor<<Self::Spec as ::sov_modules_api::Spec>::Storage>},
                },
            ],
        );
        let end_slot_hook_fn = Self::make_hooks_fn(
            fields,
            &Ident::new("kernel_end_slot_hook", Span::call_site()),
            &vec![],
            false,
            vec![
                &ArgWithType {
                    arg: Ident::new("gas_used", Span::call_site()),
                    ty: quote::quote! {&<Self::Spec as ::sov_modules_api::Spec>::Gas},
                },
                &ArgWithType {
                    arg: Ident::new("state", Span::call_site()),
                    ty: quote::quote! {&mut ::sov_modules_api::KernelStateAccessor<<Self::Spec as ::sov_modules_api::Spec>::Storage>},
                },
            ],
        );

        quote::quote! {
            use ::sov_modules_api::hooks::KernelSlotHooks;

            impl #impl_generics ::sov_modules_api::KernelSlotHooks for #ident #type_generics #where_clause {
                type Spec = <Self as ::sov_modules_api::DispatchCall>::Spec;

                #begin_slot_hook_fn

                #end_slot_hook_fn
            }
        }
    }

    fn derive_finalize_hook(
        ident: &Ident,
        fields: &[StructNamedField],
        impl_generics: &syn::ImplGenerics,
        type_generics: &syn::TypeGenerics,
        where_clause: &Option<&syn::WhereClause>,
    ) -> proc_macro2::TokenStream {
        let finalize_hook_fn = Self::make_hooks_fn(
            fields,
            &Ident::new("finalize_hook", Span::call_site()),
            &vec![],
            false,
            vec![
                &ArgWithType {
                    arg: Ident::new("root_hash", Span::call_site()),
                    ty: quote::quote! {&<<Self::Spec as ::sov_modules_api::Spec>::Storage as ::sov_modules_api::Storage>::Root},
                },
                &ArgWithType {
                    arg: Ident::new("state", Span::call_site()),
                    ty: quote::quote! {&mut impl ::sov_modules_api::AccessoryStateReaderAndWriter},
                },
            ],
        );

        quote::quote! {
            use ::sov_modules_api::hooks::FinalizeHook;

            impl #impl_generics ::sov_modules_api::FinalizeHook for #ident #type_generics #where_clause {
                type Spec = <Self as ::sov_modules_api::DispatchCall>::Spec;

                #finalize_hook_fn
            }
        }
    }

    fn derive_tx_hooks(
        ident: &Ident,
        fields: &[StructNamedField],
        impl_generics: &syn::ImplGenerics,
        type_generics: &syn::TypeGenerics,
        where_clause: &Option<&syn::WhereClause>,
    ) -> proc_macro2::TokenStream {
        let method_generics = &vec![quote::quote! {T: ::sov_modules_api::TxState<Self::Spec>}];

        let pre_dispatch_tx_hook_fn = Self::make_hooks_fn(
            fields,
            &Ident::new("pre_dispatch_tx_hook", Span::call_site()),
            method_generics,
            true,
            vec![
                &ArgWithType {
                    arg: Ident::new("tx", Span::call_site()),
                    ty: quote::quote! {&::sov_modules_api::AuthenticatedTransactionData<Self::Spec>},
                },
                &ArgWithType {
                    arg: Ident::new("state", Span::call_site()),
                    ty: quote::quote! {&mut T},
                },
            ],
        );
        let post_dispatch_tx_hook_fn = Self::make_hooks_fn(
            fields,
            &Ident::new("post_dispatch_tx_hook", Span::call_site()),
            method_generics,
            true,
            vec![
                &ArgWithType {
                    arg: Ident::new("tx", Span::call_site()),
                    ty: quote::quote! {&::sov_modules_api::AuthenticatedTransactionData<Self::Spec>},
                },
                &ArgWithType {
                    arg: Ident::new("context", Span::call_site()),
                    ty: quote::quote! {&::sov_modules_api::Context<Self::Spec>},
                },
                &ArgWithType {
                    arg: Ident::new("state", Span::call_site()),
                    ty: quote::quote! {&mut T},
                },
            ],
        );

        quote::quote! {
            use ::sov_modules_api::hooks::TxHooks;

            impl #impl_generics ::sov_modules_api::TxHooks for #ident #type_generics #where_clause {
                type Spec = <Self as ::sov_modules_api::DispatchCall>::Spec;

                #pre_dispatch_tx_hook_fn

                #post_dispatch_tx_hook_fn
            }
        }
    }

    fn make_hooks_fn(
        fields: &[StructNamedField],
        method: &Ident,
        method_generics: &Vec<TokenStream>,
        is_faillible: bool,
        args: Vec<&ArgWithType>,
    ) -> proc_macro2::TokenStream {
        let args_names = args
            .iter()
            .map(|ArgWithType { arg, .. }| quote::quote! { #arg });

        let args_with_types = args
            .iter()
            .map(|ArgWithType { ty, arg }| quote::quote! { #arg: #ty });

        let idents = fields.iter().enumerate().map(|(i, field)| {
            let ident = &field.ident;

            quote::quote! {
                (&self.#ident, #i)
            }
        });

        let method_generics = if !method_generics.is_empty() {
            Some(quote::quote! { <#(#method_generics),*> })
        } else {
            None
        };

        let method_output_ty = if is_faillible {
            quote::quote! {::sov_modules_api::prelude::anyhow::Result<()>}
        } else {
            quote::quote! {()}
        };

        let matches = fields.iter().enumerate().map(|(i, field)| {
            let ident = &field.ident;
            let args_loop = args_names.clone();

            let module_call = if is_faillible {
                quote::quote! {(&self.#ident).#method(#(#args_loop),*)?}
            } else {
                quote::quote! {(&self.#ident).#method(#(#args_loop),*)}
            };

            quote::quote! {
                #i => #module_call,
            }
        });

        let output = if is_faillible {
            Some(quote::quote! {::std::result::Result::Ok(())})
        } else {
            None
        };

        quote::quote! {
            fn #method #method_generics (&self, #(#args_with_types),*) -> #method_output_ty {
                let modules: ::std::vec::Vec<(&dyn ::sov_modules_api::ModuleInfo<Spec = Self::Spec>, usize)> = ::std::vec![#(#idents),*];

                let sorted_modules = ::sov_modules_api::sort_values_by_modules_dependencies(modules).expect("Sorting of modules failed");

                for module in sorted_modules {
                     match module {
                         #(#matches)*
                         _ => panic!("Module not found: {:?}", module),
                     }
                };

                #output
            }
        }
    }
}
