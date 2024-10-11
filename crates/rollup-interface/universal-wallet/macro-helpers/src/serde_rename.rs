use convert_case::{Case, Casing};
use darling::ast::NestedMeta;
use darling::util::{parse_attribute_to_meta_list, WithOriginal};
use darling::FromMeta;
use quote::ToTokens;
use syn::{Ident, Meta};

#[derive(Debug, Default, FromMeta)]
pub struct ForeignAttrs {
    serde: SerdeRename,
}

#[derive(Debug, Default, FromMeta)]
#[darling(allow_unknown_fields)]
pub struct SerdeRename {
    /// used by serde on structs and variants
    rename_all: Option<WithOriginal<String, Meta>>,
    // TODO: these aren't currently used in the SDK and can be implemented later if needed
    // rename_all_fields: Option<String>, // used by serde on structs
    //rename: Option<String>, // used by serde on structs, variants and individual fields
}

/// This is an MVP implementation that handles the rename_all attribute, but not `rename`
/// attributes on fields.
/// To add support to `rename`, attribute parsing should also be extended to `InputField`s, and
/// then some extra logic needs to be added to construct a merged `SerdeRename` from the parent
/// type's one (where `rename_all = Some(...)`) and the one from the field (where `rename =
/// Some(...)`). The latter should override the former.
impl SerdeRename {
    fn rename_using_rename_all(&self, original: &Ident) -> Result<String, syn::Error> {
        match &self.rename_all {
            Some(str) if str.parsed == "snake_case" => {
                Ok(original.to_string().to_case(Case::Snake))
            }
            Some(str) => Err(syn::Error::new_spanned(
                &str.original,
                "Rename_all option is unsupported in UniversalWallet",
            )),
            _ => Ok(original.to_string()),
        }
    }

    pub fn rename_field(&self, original: &Ident) -> Result<String, syn::Error> {
        self.rename_using_rename_all(original)
    }

    pub fn rename_variant(&self, original: &Ident) -> Result<String, syn::Error> {
        self.rename_using_rename_all(original)
    }

    pub fn rename_typename(&self, original: &Ident) -> Result<String, syn::Error> {
        Ok(original.to_string()) // TODO: implement the `rename` attribute
    }
}

pub fn parse_serde_rename_attrs(attrs: Vec<syn::Attribute>) -> darling::Result<SerdeRename> {
    let mut res = ForeignAttrs::default();
    for a in attrs.iter() {
        if a.path().is_ident("serde") {
            let meta_list = parse_attribute_to_meta_list(a)?;
            let list = NestedMeta::parse_meta_list(meta_list.into_token_stream())?;
            let parsed_attr = ForeignAttrs::from_list(&list)?.serde;
            // TODO: better logic here if parsing multiple attributes
            if parsed_attr.rename_all.is_some() {
                res.serde.rename_all = parsed_attr.rename_all;
            }
        }
    }
    Ok(res.serde)
}
