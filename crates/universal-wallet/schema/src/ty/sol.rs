use std::iter;

use crate::{
    schema::{IndexLinking, Schema},
    ty::{
        visitor::{ResolutionError, TypeResolver},
        Ty,
    },
};
use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum Error {
    #[error(transparent)]
    UnresolvedType(#[from] ResolutionError),
}

pub mod ast;

impl Ty<IndexLinking> {
    pub fn as_inline_type(&self, schema: &Schema) -> Result<ast::Ty, Error> {
        let ty = match self {
            Ty::Integer(_, _) => ast::Ty::Uint256,
            Ty::ByteVec { .. } => ast::Ty::Bytes,
            Ty::String => ast::Ty::String,
            Ty::Struct(s) => ast::Ty::Ident(s.type_name.clone()),
            Ty::Option { .. } => unimplemented!("Options are not supported"),
            Ty::Vec { value } => {
                let ty = schema.resolve_or_err(value)?.as_inline_type(schema)?;
                ast::Ty::DynamicArray(Box::new(ty))
            }
            Ty::Array { len, value } => {
                let ty = schema.resolve_or_err(value)?.as_inline_type(schema)?;
                ast::Ty::Array(Box::new(ty), *len)
            }
            Ty::ByteArray { len, .. } if *len <= 32 => ast::Ty::ByteArray(*len as u8),
            Ty::ByteArray { .. } => panic!("Solidity only supports byte arrays of len up to 32"),
            Ty::Tuple(t) => {
                if t.fields.len() == 1 {
                    schema
                        .resolve_or_err(&t.fields[0].value)?
                        .as_inline_type(schema)?
                } else {
                    unimplemented!("We don't support multi-element tuples. Please use structs")
                }
            }
            Ty::Enum(_) => unimplemented!("Enums are only supported on the top level"),
            _ => {
                todo!()
            }
        };
        Ok(ty)
    }

    pub fn as_definitions(&self, schema: &Schema) -> Result<Definitions, Error> {
        match self {
            Ty::Struct(s) => {
                let mut auxilary = vec![];
                let mut fields = vec![];
                for field in &s.fields {
                    let field_ty = schema.resolve_or_err(&field.value)?;

                    let nested_definitions = field_ty.as_definitions(schema)?;
                    auxilary.extend(nested_definitions);

                    let field =
                        ast::Field::new(&field.display_name, field_ty.as_inline_type(schema)?);
                    fields.push(field);
                }
                let definition = ast::Struct::new(&s.type_name, fields);
                Ok(Definitions::new([definition], auxilary))
            }
            Ty::Enum(e) => {
                let mut auxilary = vec![];
                let mut top_level = vec![];
                for variant in &e.variants {
                    if let Some(ref value) = variant.value {
                        let variant_ty = schema.resolve_or_err(value)?;
                        let mut definitions = variant_ty.as_definitions(schema)?;
                        if definitions.top_level.len() == 0 {
                            let fields =
                                vec![ast::Field::new("_0", variant_ty.as_inline_type(schema)?)];
                            let definition = ast::Struct::new(
                                format!("{}_{}", e.type_name, variant.name),
                                fields,
                            );
                            top_level.push(definition);
                        } else if definitions.top_level.len() == 1 {
                            definitions.top_level.iter_mut().for_each(|def| {
                                def.name = format!("{}_{}", e.type_name, variant.name)
                            });
                        } else {
                            definitions.top_level.iter_mut().for_each(|def| {
                                def.prepend_context(&format!("{}_{}", e.type_name, variant.name))
                            });
                        }

                        top_level.extend(definitions.top_level);
                        auxilary.extend(definitions.auxilary);
                    } else {
                        // Solidity does not support empty struct so we convert enum variants with no associated data into `{ bool _phantom }`
                        let fields = vec![ast::Field::new("_phantom", ast::Ty::Bool)];
                        let definition =
                            ast::Struct::new(format!("{}_{}", e.type_name, variant.name), fields);
                        top_level.push(definition);
                    };
                }
                Ok(Definitions::new(top_level, auxilary))
            }
            Ty::Tuple(t) => {
                if t.fields.len() == 1 {
                    schema
                        .resolve_or_err(&t.fields[0].value)?
                        .as_definitions(schema)
                } else {
                    unimplemented!("We don't support multi-element tuples. Please use structs")
                }
            }
            Ty::Array { value, .. } | Ty::Vec { value } => {
                let item_ty = schema.resolve_or_err(value)?;
                item_ty.as_definitions(schema)
            }
            Ty::Option { .. } => unimplemented!("Options are not supported"),
            Ty::Integer(_, _)
            | Ty::ByteArray { .. }
            | Ty::ByteVec { .. }
            | Ty::String
            | Ty::Float32
            | Ty::Float64
            | Ty::Boolean
            | Ty::Skip { .. } => Ok(Definitions::default()), // Primitives don't need to be defined
            Ty::Map { .. } => todo!(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Definitions {
    top_level: Vec<ast::Struct>,
    auxilary: Vec<ast::Struct>,
}

impl Definitions {
    fn new(
        top_level: impl IntoIterator<Item = ast::Struct>,
        auxilary: impl IntoIterator<Item = ast::Struct>,
    ) -> Self {
        Self {
            top_level: top_level.into_iter().collect(),
            auxilary: auxilary.into_iter().collect(),
        }
    }
}

impl IntoIterator for Definitions {
    type Item = ast::Struct;
    type IntoIter = iter::Chain<std::vec::IntoIter<ast::Struct>, std::vec::IntoIter<ast::Struct>>;

    fn into_iter(self) -> Self::IntoIter {
        self.top_level.into_iter().chain(self.auxilary.into_iter())
    }
}

#[cfg(test)]
mod tests {
    use sov_universal_wallet::schema::Schema;
    use sov_universal_wallet::ty::sol::ast::{Block, Field, Struct, Ty::*};
    use sov_universal_wallet::UniversalWallet;

    // Hack - because the macro is configured to be re-exported from sov_rollup_interface;
    // but _we_ are a dependency of sov_rollup_interface so we can't import it without causing a cycle
    // This should not be an issue anywhere else except inside this crate's tests right here
    mod sov_rollup_interface {
        pub use sov_universal_wallet;
    }

    #[derive(UniversalWallet, borsh::BorshSerialize, borsh::BorshDeserialize)]
    struct TestStruct {
        field: u8,
    }

    #[test]
    fn test_struct() -> anyhow::Result<()> {
        let schema = Schema::of_single_type::<TestStruct>()?;
        let definitions = schema.into_alloy()?;
        let expected = Block(vec![Struct::new(
            "TestStruct",
            [Field::new("field", Uint256)],
        )]);
        assert_eq!(definitions, expected);
        Ok(())
    }

    #[derive(UniversalWallet, borsh::BorshSerialize, borsh::BorshDeserialize)]
    enum Enum {
        Empty,
        Tuple(u8),
        Struct { field: u8 },
    }

    #[test]
    fn test_enum() -> anyhow::Result<()> {
        let schema = Schema::of_single_type::<Enum>()?;
        let definitions = schema.into_alloy()?;

        let expected = Block(vec![
            Struct::new("Enum_Empty", [Field::new("_phantom", Bool)]),
            Struct::new("Enum_Tuple", [Field::new("_0", Uint256)]),
            Struct::new("Enum_Struct", [Field::new("field", Uint256)]),
        ]);
        assert_eq!(definitions, expected);
        Ok(())
    }

    #[derive(UniversalWallet, borsh::BorshSerialize, borsh::BorshDeserialize)]
    enum EnumWithExternalStruct {
        Struct(TestStruct),
    }

    #[test]
    fn test_enum_with_external_struct() -> anyhow::Result<()> {
        let schema = Schema::of_single_type::<EnumWithExternalStruct>()?;
        let definitions = schema.into_alloy()?;

        let expected = Block(vec![Struct::new(
            "EnumWithExternalStruct_Struct",
            [Field::new("field", Uint256)],
        )]);
        assert_eq!(definitions, expected);
        Ok(())
    }

    #[derive(UniversalWallet, borsh::BorshSerialize, borsh::BorshDeserialize)]
    struct InnerTuple(pub u8);

    #[derive(UniversalWallet, borsh::BorshSerialize, borsh::BorshDeserialize)]
    struct NestedTuples(pub InnerTuple);

    #[test]
    fn test_nested_tuples() -> anyhow::Result<()> {
        let schema = Schema::of_single_type::<NestedTuples>()?;
        let definitions = schema.into_alloy()?;

        let expected = Block(vec![]);
        assert_eq!(definitions, expected);
        Ok(())
    }

    #[derive(UniversalWallet, borsh::BorshSerialize, borsh::BorshDeserialize)]
    enum Inner {
        Yes,
        No,
    }

    #[derive(UniversalWallet, borsh::BorshSerialize, borsh::BorshDeserialize)]
    enum Call {
        Bank(Inner),
        ValueSetter(Inner),
    }

    #[test]
    fn test_enum_within_enum() -> anyhow::Result<()> {
        let schema = Schema::of_single_type::<Call>()?;
        let definitions = schema.into_alloy()?;

        let expected = Block(vec![
            Struct::new("Call_Bank_Inner_Yes", [Field::new("_phantom", Bool)]),
            Struct::new("Call_Bank_Inner_No", [Field::new("_phantom", Bool)]),
            Struct::new("Call_ValueSetter_Inner_Yes", [Field::new("_phantom", Bool)]),
            Struct::new("Call_ValueSetter_Inner_No", [Field::new("_phantom", Bool)]),
        ]);
        assert_eq!(definitions, expected);
        Ok(())
    }
}
