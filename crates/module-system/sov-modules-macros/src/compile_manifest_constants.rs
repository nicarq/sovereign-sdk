use proc_macro::TokenStream;
use syn::{Ident, LitStr};

use crate::manifest::Manifest;

pub fn make_const_value(constant_name: &syn::LitStr) -> syn::Result<proc_macro2::TokenStream> {
    let field_ident = Ident::new(&constant_name.value(), constant_name.span());
    Manifest::read_constants(&field_ident)?.parse_expression(&field_ident)
}

fn parse_gas_dimensions(
    constant_name: &syn::LitStr,
    manifest: &Manifest,
) -> Result<usize, syn::Error> {
    let gas_dimensions_string = String::from("GAS_DIMENSIONS");
    let lit_str = LitStr::new(&gas_dimensions_string, constant_name.span());

    let expr_from_config_file = make_const_value(&lit_str).map_err(|_| {
        syn::Error::new(
            constant_name.span(),
            format!(
                "Impossible to get the number of gas dimensions from the config constant file. The constant named `{}` inside `{}` does not exist.",
                gas_dimensions_string, manifest.path().display()
            ),
        )
    })?;

    let gas_dimensions = syn::parse::<syn::ExprLit>(expr_from_config_file.clone().into())
        .map_err(|_| {
            syn::Error::new(
                constant_name.span(),
                format!(
                "The constant named `{}` inside `{}` represents a number of gas dimensions - it should be an expression litteral. Instead, it is: `{}`",
                    gas_dimensions_string, manifest.path().display(), expr_from_config_file
                ),
            )
        })?;

    match gas_dimensions.lit {
        syn::Lit::Int(lit) => lit.base10_parse::<usize>().map_err(|_| {
            syn::Error::new(
                constant_name.span(),
                format!(
                    "The constant named `{}` inside `{}` is not a valid number of gas dimensions. It should be an integer expressed in base 10. Instead, it is: `{}`",
                    gas_dimensions_string, manifest.path().display(), expr_from_config_file
                ),
            )
        }),
        _ => {
            Err(syn::Error::new(
                constant_name.span(),
                format!(
                    "The constant named `{}` inside `{}` is not a valid number of gas dimensions. It should be an integer. Instead, it is: `{}`",
                    gas_dimensions_string, manifest.path().display(), expr_from_config_file
                ),
            ))
        }
    }
}

/// Helper function to parse a gas constant. Returns the inner expression and the length of the associated array.
fn parse_const_gas(constant_name: &syn::LitStr) -> Result<(syn::ExprArray, usize), syn::Error> {
    // Parse the constant from the configuration file, and convert it to a Rust
    // expression...
    let field_ident = Ident::new(&constant_name.value(), constant_name.span());
    let manifest = Manifest::read_constants(&field_ident)?;
    let expr_from_config_file = make_const_value(constant_name)?;

    let gas_array_expr = syn::parse::<syn::ExprArray>(expr_from_config_file.clone().into())
        .map_err(|_| {
            syn::Error::new(
                constant_name.span(),
                format!(
                "The constant named `{}` inside `{}` represents a gas unit - it should be an array of integers. It's not an array in the first place: `{}`",
                constant_name.value(), manifest.path().display(), expr_from_config_file
            ),
            )
        })?;

    let len = gas_array_expr.elems.len();

    let gas_dimensions = parse_gas_dimensions(constant_name, &manifest)?;

    if gas_dimensions != len {
        return Err(syn::Error::new(
            constant_name.span(),
            format!(
                "The constant named `{}` inside `{}` has {} dimensions, but the number of gas dimensions is {}. The number of gas dimensions should be the same as the number of dimensions of the gas unit.",
                constant_name.value(), manifest.path().display(), len, gas_dimensions
            ),
        ));
    }

    Ok((gas_array_expr, len))
}

pub fn make_const_gas_price(constant_name: &syn::LitStr) -> syn::Result<TokenStream> {
    let (gas_array_expr, len) = parse_const_gas(constant_name)?;
    Ok(quote::quote! {
        GasPrice::<#len>::from_primitive( #gas_array_expr )
    }
    .into())
}

pub fn make_const_gas_unit(constant_name: &syn::LitStr) -> syn::Result<TokenStream> {
    let (gas_array_expr, len) = parse_const_gas(constant_name)?;
    Ok(quote::quote! {
        GasUnit::<#len>::from_primitive( #gas_array_expr )
    }
    .into())
}

pub fn make_const_bech32(
    constant_name: &syn::LitStr,
    bech32_wrapper_type: &syn::Type,
) -> syn::Result<TokenStream> {
    use bech32::primitives::decode::CheckedHrpstring;
    use bech32::Bech32m;

    // Parse the constant from the configuration file, and convert it to a Rust
    // expression...
    let field_ident = Ident::new(&constant_name.value(), constant_name.span());
    let manifest = Manifest::read_constants(&field_ident)?;
    let expr_from_config_file = make_const_value(constant_name)?;

    // ...and parse the expression as a string literal.
    let bech32_lit_str =
        syn::parse::<syn::LitStr>(expr_from_config_file.clone().into()).map_err(|_| {
            syn::Error::new(
                constant_name.span(),
                format!(
                "The constant named `{}` inside `{}` should be a bech32 string, but it's not a string in the first place: `{}`",
                constant_name.value(), manifest.path().display(), expr_from_config_file
            ),
            )
        })?;
    let bech32_string = bech32_lit_str.value();

    // Parse the string literal as a bech32 string.
    let type_name = quote::quote! { #bech32_wrapper_type };
    let hrp_string = CheckedHrpstring::new::<Bech32m>(&bech32_string).map_err(|err| {
        syn::Error::new(
            constant_name.span(),
            format!(
                "The constant named `{}` inside `{}` is not a valid bech32 string; decoding failed: {}",
                constant_name.value(),
                manifest.path().display(),
                err
            ),
        )
    })?;
    let bech32_bytes = hrp_string.byte_iter();
    let found_prefix = hrp_string.hrp().to_string();

    let const_expr_tokens = {
        // Generate the code which will check that the HRP (Human-Readable Part) is correct.
        let bad_prefix_error = format!(
            "The constant named `{}` with value `{}` inside `{}` is a valid bech32 string but its prefix `{}` does not match the expected prefix for the type `{}`.",
            constant_name.value(),
            bech32_string,
            manifest.path().display(),
            found_prefix,
            type_name
        );

        // We can't loop in a `const fn`, so we generate an `assert!` for each byte.
        let mut assertions = vec![];
        for (idx, byte) in found_prefix.as_bytes().iter().enumerate() {
            assertions.push(quote::quote! {
                assert!(#byte == #found_prefix.as_bytes()[#idx], #bad_prefix_error);
            });
        }

        quote::quote!({
            if #found_prefix.len() != #type_name::bech32_prefix().len() {
                panic!(#bad_prefix_error);
            }
            #(#assertions)*

            #bech32_wrapper_type::from_const_slice( [#(#bech32_bytes,)*] )
        })
    };

    Ok(const_expr_tokens.into())
}
