use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse2, parse_quote, Block, Ident, ItemFn};

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
