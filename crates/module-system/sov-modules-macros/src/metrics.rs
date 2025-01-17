use proc_macro::TokenStream;
use quote::ToTokens;
use syn::{parse2, parse_quote, Block, Ident, ItemFn};

#[cfg(all(feature = "gas-constant-estimation", feature = "native"))]
pub mod gas_estimation {
    use quote::quote;

    use super::*;

    /// Wrap a block with benchmarking. Fills the correct cycle counter based on the target and vendor.
    pub fn const_tracker(ident: &Ident, block: &Block, tagged_inputs: Vec<Ident>) -> Box<Block> {
        let inputs_iter = tagged_inputs
            .iter()
            .map(|ident| quote! { (stringify!(#ident).to_string(), #ident.to_string()) })
            .collect::<Vec<_>>();

        let const_tracker_block: Box<Block> = parse_quote! {
            {
                let inputs = vec![ #(#inputs_iter,)* ];

                let constants_usage_before: ::sov_metrics::GasConstantTracker =
                    ::sov_metrics::GAS_CONSTANTS.with(|gas_constants| gas_constants.borrow().clone());
                let result = (|| #block)();
                let constants_usage_after: ::sov_metrics::GasConstantTracker =
                    ::sov_metrics::GAS_CONSTANTS.with(|gas_constants| gas_constants.borrow().clone());

                let constants_usage_diff = constants_usage_after.diff(constants_usage_before);

                constants_usage_diff.report_gas_constants_usage(stringify!(#ident), inputs);

                result
            }
        };

        parse_quote!({
            #[cfg(feature="gas-constant-estimation")] return #const_tracker_block;
            #[cfg(not(feature="gas-constant-estimation"))]
            #block
        })
    }
}

#[cfg(feature = "bench")]
pub mod zk {
    use quote::quote;

    use super::*;

    /// Wrap a block with benchmarking. Fills the correct cycle counter based on the target and vendor.
    pub fn cycles(ident: &Ident, block: &Block, tagged_inputs: Vec<Ident>) -> Box<Block> {
        let risc0_zk_block = cycles_inner_risc0(ident, block, &tagged_inputs);
        let sp1_zk_block = cycles_inner_sp1(ident, block, &tagged_inputs);

        parse_quote!({
            #[cfg(all(target_os = "zkvm", target_vendor = "succinct"))]
            {
                return #sp1_zk_block;
            }
            #[cfg(all(target_os = "zkvm", target_vendor = "risc0"))]
            {
                return #risc0_zk_block;
            }
            #[cfg(not(target_os = "zkvm"))]
            {
                return #block;
            }
        })
    }

    fn cycles_inner_risc0(ident: &Ident, block: &Block, tagged_inputs: &[Ident]) -> Box<Block> {
        let inputs_iter = tagged_inputs
            .iter()
            .map(|ident| quote! { (stringify!(#ident).to_string(), #ident.to_string()) })
            .collect::<Vec<_>>();

        parse_quote! {
            {
                let inputs = vec![ #(#inputs_iter,)* ];

                let cycles_before = ::sov_metrics::cycle_utils::risc0::get_cycle_count();
                let result = (|| #block)();
                let cycles_after = ::sov_metrics::cycle_utils::risc0::get_cycle_count();
                let heap_bytes_free_after = ::sov_metrics::cycle_utils::risc0::get_available_heap();

                let cycles = cycles_after.saturating_sub(cycles_before);
                ::sov_metrics::cycle_utils::risc0::report_cycle_count(
                    ::sov_metrics::cycle_utils::CycleMetric {
                        name: stringify!(#ident).to_string(),
                        metadata: inputs,
                        count: cycles,
                        free_heap_bytes: heap_bytes_free_after,
                    }
                );

                result
            }
        }
    }

    fn cycles_inner_sp1(ident: &Ident, block: &Block, tagged_inputs: &[Ident]) -> Box<Block> {
        let inputs_iter = tagged_inputs
            .iter()
            .map(|ident| quote! { (stringify!(#ident).to_string(), #ident.to_string()) })
            .collect::<Vec<_>>();

        parse_quote!({
           {
                let inputs = vec![ #(#inputs_iter,)* ];

                let before = ::sov_metrics::cycle_utils::sp1::get_cycle_count();
                let result = (move || #block)();
                let after = ::sov_metrics::cycle_utils::sp1::get_cycle_count();
                let heap_bytes_free_after = ::sov_metrics::cycle_utils::sp1::get_available_heap();

                ::sov_metrics::cycle_utils::sp1::report_cycle_count(
                    ::sov_metrics::cycle_utils::CycleMetric {
                        name: stringify!(#ident).to_string(),
                        metadata: inputs,
                        count: after - before,
                        free_heap_bytes: heap_bytes_free_after,
                    });
                result
            }
        })
    }
}

/// Wrap a function, where `f` wraps a block with (benchmarking) code.
pub fn wrap_function_with<F>(
    f: F,
    input: TokenStream,
    tag_attr: Vec<Ident>,
) -> Result<TokenStream, syn::Error>
where
    F: Fn(&Ident, &Block, Vec<Ident>) -> Box<Block>,
{
    let mut input = parse2::<ItemFn>(input.into())?;
    let ItemFn {
        sig: syn::Signature { ident, inputs, .. },
        block,
        ..
    } = input.clone();

    // We are collecting the function inputs idents that have been tagged in the macro arguments.
    // For that we are looping over the function arguments and checking that the associated identifier
    // is part of the set of macro arguments.
    let tagged_inputs = inputs
        .into_iter()
        .filter_map(|i| match i {
            syn::FnArg::Typed(syn::PatType { pat, .. }) => match *pat {
                syn::Pat::Ident(pat_ident) if tag_attr.contains(&pat_ident.ident) => {
                    Some(pat_ident.ident)
                }
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();

    input.block = f(&ident, &block, tagged_inputs);

    Ok(input.to_token_stream().into())
}
