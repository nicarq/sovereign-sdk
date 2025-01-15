use proc_macro::TokenStream;
use quote::ToTokens;
use syn::{parse2, parse_quote, Block, Ident, ItemFn};

/// Wrap a block with benchmarking. Fills the correct cycle counter based on the target and vendor.
pub fn const_tracker(ident: &Ident, block: &Block) -> Box<Block> {
    let const_tracker_block = track_constants(ident, block);

    parse_quote!({
        #[cfg(feature="gas-constant-estimation")] return #const_tracker_block;
        #[cfg(not(feature="gas-constant-estimation"))]
        #block
    })
}

fn track_constants(ident: &Ident, block: &Block) -> Box<Block> {
    parse_quote! {
        {
            let constants_usage_before: ::sov_metrics::GasConstantTracker =
                ::sov_metrics::GAS_CONSTANTS.with(|gas_constants| gas_constants.borrow().clone());
            let result = (|| #block)();
            let constants_usage_after: ::sov_metrics::GasConstantTracker =
                ::sov_metrics::GAS_CONSTANTS.with(|gas_constants| gas_constants.borrow().clone());

            let constants_usage_diff = constants_usage_after.diff(constants_usage_before);

            constants_usage_diff.report_gas_constants_usage(stringify!(#ident));

            result
        }
    }
}

/// Wrap a function, where `f` wraps a block with (benchmarking) code.
pub fn wrap_function_with<F>(f: F, input: TokenStream) -> Result<TokenStream, syn::Error>
where
    F: Fn(&Ident, &Block) -> Box<Block>,
{
    let mut input = parse2::<ItemFn>(input.into())?;
    let ItemFn {
        sig: syn::Signature { ident, .. },
        block,
        ..
    } = &input;
    input.block = f(ident, block);

    Ok(input.to_token_stream().into())
}
