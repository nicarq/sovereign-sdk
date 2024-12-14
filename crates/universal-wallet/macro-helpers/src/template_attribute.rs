use std::collections::HashMap;

use darling::{Error, FromMeta, Result};
use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::parse::Parser;
use syn::{Expr, ExprLit, Lit, Meta, Token};

/// Attributes of the format
/// `#[sov_wallet(template("transfer" = value("hi"), "other_template" = input("msg")))]`
/// providing the annotations necessary to generate SchemaGenerator::get_child_templates() for
/// fields which are part of standard templated transactions
#[derive(Debug, Default, Clone)]
pub struct TransactionTemplates {
    pub original: TokenStream,
    pub template: HashMap<String, InputOrValue>,
}

#[derive(Debug, Clone)]
pub enum InputOrValue {
    Input(String),
    Value(String),
}

impl FromMeta for TransactionTemplates {
    /// This is parsed as a Punctuated of individual templates, each consisting of an Expr::Assign
    /// with the template name on the LHS and an Expr::Call for the InputOrValue on the RHS
    ///
    /// We need to override `from_meta` directly instead of using e.g. `from_list` because
    /// darling's parsing seems pretty pretty rigid. For example, `from_list` tries to parse it as
    /// `Punctuated::<NestedMeta, Token![,]>` which is too greedy, fails to parse the complex Expr
    /// and complains that it expects ',' at the first '='.
    fn from_meta(item: &Meta) -> Result<Self> {
        fn parse_lit_str(e: Expr) -> Result<String> {
            match e {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) => Ok(s.value()),
                Expr::Lit(expr) => Err(Error::unexpected_lit_type(&expr.lit)),
                _ => Err(Error::unexpected_expr_type(&e).with_span(&e)),
            }
        }

        // Split it into a list of templates; each expr is an individual template annotation
        let list = match item {
            Meta::List(list) => Ok(list),
            Meta::Path(_) => Err(Error::unsupported_format("Path").with_span(item)),
            Meta::NameValue(_) => Err(Error::unsupported_format("NameValue").with_span(item)),
        }?;
        let exprs = syn::punctuated::Punctuated::<Expr, Token![,]>::parse_terminated
            .parse2(list.tokens.clone())?;

        let mut ret = HashMap::new();

        for expr in exprs {
            match expr {
                Expr::Assign(ref expr) => {
                    // LHS of assignment is template name
                    let template_name = parse_lit_str((*expr.left).clone())?;
                    if ret.contains_key(&template_name) {
                        return Err(Error::duplicate_field(&template_name).with_span(&expr));
                    }

                    // RHS is either `input("name binding")` or `value("default value")`
                    let template_binding = match *expr.right {
                        Expr::Call(ref c) => Ok(c),
                        // TODO: support for Path here for `input` - to reuse typename as input name
                        _ => Err(Error::unexpected_expr_type(&expr.right).with_span(&expr.right)),
                    }?;
                    let discriminant = match *template_binding.func {
                        Expr::Path(ref p) => p.path.get_ident(),
                        _ => None,
                    }
                    .ok_or(
                        Error::unexpected_expr_type(&template_binding.func)
                            .with_span(&template_binding.func),
                    )?;

                    // And we need exactly one string value between the parentheses
                    // Also, for some reason, ExprCall.args doesn't have the right span
                    // information, so we set the errors to the ExprCall itself
                    match template_binding.args.len().cmp(&1) {
                        std::cmp::Ordering::Less => {
                            return Err(Error::too_few_items(1).with_span(&template_binding))
                        }
                        std::cmp::Ordering::Greater => {
                            return Err(Error::too_many_items(1).with_span(&template_binding))
                        }
                        _ => (),
                    };
                    // TODO: support for Assign values here for `borsh = ""` pre-encoded values
                    // TODO: support for Path values for `default` to encode a call to Default::default()
                    let input_or_value = parse_lit_str(template_binding.args[0].clone())?;

                    match discriminant.to_string().as_str() {
                        "input" => ret.insert(template_name, InputOrValue::Input(input_or_value)),
                        "value" => ret.insert(template_name, InputOrValue::Value(input_or_value)),
                        s => return Err(Error::unknown_field(s).with_span(discriminant)),
                    };

                    Ok(())
                }
                _ => Err(Error::unexpected_expr_type(&expr).with_span(&expr)),
            }?;
        }
        Ok(TransactionTemplates {
            template: ret,
            original: item.to_token_stream(),
        })
    }
}
