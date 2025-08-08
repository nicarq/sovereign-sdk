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
            Ty::String { .. } => ast::Ty::String,
            Ty::Struct(s) => ast::Ty::Ident(s.type_name.clone()),
            Ty::Option { value } => schema.resolve_or_err(value)?.as_inline_type(schema)?, // TODO: For now I resolve Option<T> as T
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
                    // TODO: Proper tuple support. For now just supporting the case where (T) is treated as T. One element tuples are used in Gas types
                    let field_type = schema.resolve_or_err(&t.fields[0].value)?;
                    field_type.as_inline_type(schema)?
                } else {
                    todo!()
                }
            }
            _ => {
                todo!()
            }
        };
        Ok(ty)
    }

    pub fn as_definitions(&self, schema: &Schema) -> Result<Vec<ast::Struct>, Error> {
        let mut definitions = vec![];
        match self {
            Ty::Struct(s) => {
                let mut fields = vec![];
                for field in &s.fields {
                    let field_ty = schema.resolve_or_err(&field.value)?;

                    let nested_definitions = field_ty.as_definitions(schema)?;
                    definitions.extend(nested_definitions);

                    let field =
                        ast::Field::new(&field.display_name, field_ty.as_inline_type(schema)?);
                    fields.push(field);
                }
                let definition = if s.type_name.starts_with("__SovVirtualWallet") {
                    ast::Struct::synthetic("", fields) // It's caller's responsibility to add context to a name
                } else {
                    ast::Struct::native(&s.type_name, fields)
                };
                definitions.push(definition);
            }
            Ty::Enum(e) => {
                for variant in &e.variants {
                    if let Some(ref value) = variant.value {
                        let variant_ty = schema.resolve_or_err(value)?;
                        let nested_definitions = variant_ty.as_definitions(schema)?;
                        let renamed_definitions = prepend_context(
                            prepend_context(nested_definitions, &variant.name),
                            &e.type_name,
                        );
                        definitions.extend(renamed_definitions);
                    } else {
                        // Solidity does not support empty struct so we convert enum variants with no associated data into `{ bool _phantom }`
                        let field = ast::Field::new("_phantom", ast::Ty::Bool);
                        let definition = ast::Struct::synthetic(&variant.name, [field]);
                        definitions.push(definition.prepend_context(&e.type_name));
                    }
                }
            }
            Ty::Tuple(t) => {
                let mut fields = vec![];
                for (idx, field) in t.fields.iter().enumerate() {
                    let field_ty = schema.resolve_or_err(&field.value)?;

                    let nested_definitions = field_ty.as_definitions(schema)?;
                    definitions.extend(nested_definitions);

                    let field =
                        ast::Field::new(format!("_{idx}"), field_ty.as_inline_type(schema)?);
                    fields.push(field);
                }
                let definition = ast::Struct::synthetic("", fields);
                definitions.push(definition);
            }
            Ty::Array { value, .. } | Ty::Vec { value } => {
                let item_ty = schema.resolve_or_err(value)?;
                let nested_definitions = item_ty.as_definitions(schema)?;
                definitions.extend(nested_definitions);
            }
            Ty::Option { .. } => (), // TODO: Fow now Option<T> is treated as T so no new definitions needed
            Ty::Integer(_, _)
            | Ty::ByteArray { .. }
            | Ty::ByteVec { .. }
            | Ty::String
            | Ty::Float32
            | Ty::Float64
            | Ty::Boolean
            | Ty::Skip { .. } => (), // Primitives don't need to be defined
            Ty::Map { .. } => todo!(),
        };
        Ok(definitions)
    }
}

fn prepend_context(definitions: Vec<ast::Struct>, context: &str) -> Vec<ast::Struct> {
    definitions
        .into_iter()
        .map(|d| d.prepend_context(context))
        .collect()
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
        let expected = Block(vec![Struct::native(
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
            Struct::synthetic("Enum_Empty", [Field::new("_phantom", Bool)]),
            Struct::synthetic("Enum_Tuple", [Field::new("_0", Uint256)]),
            Struct::synthetic("Enum_Struct", [Field::new("field", Uint256)]),
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

        let expected = Block(vec![
            Struct::native("TestStruct", [Field::new("field", Uint256)]),
            Struct::synthetic(
                "EnumWithExternalStruct_Struct",
                [Field::new("_0", Ident("TestStruct".into()))],
            ),
        ]);
        assert_eq!(definitions, expected);
        Ok(())
    }
}
