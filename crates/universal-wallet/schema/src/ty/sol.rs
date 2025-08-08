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
            Ty::Struct(s) => ast::Ty::Ident(s.type_name.clone()),
            Ty::Option { value } => schema.resolve_or_err(value)?.as_inline_type(schema)?, // TODO: For now I resolve Option<T> as T
            Ty::Array { len, value } => {
                let ty = schema.resolve_or_err(value)?.as_inline_type(schema)?;
                ast::Ty::Array(Box::new(ty), *len)
            }
            Ty::Tuple(t) => {
                if t.fields.len() == 1 {
                    // TODO: Proper tuple support. For now just supporting the case where (T) is treated as T. One element tuples are used in Gas types
                    let field_type = schema.resolve_or_err(&t.fields[0].value)?;
                    field_type.as_inline_type(schema)?
                } else {
                    todo!()
                }
            }
            Ty::ByteVec { .. } => ast::Ty::Bytes,
            _ => todo!(),
        };
        Ok(ty)
    }

    pub fn as_definitions(&self, schema: &Schema) -> Result<Vec<ast::Struct>, Error> {
        let mut result = vec![];
        match self {
            Ty::Struct(s) => {
                let fields: Vec<_> = s
                    .fields
                    .iter()
                    .map(|field| {
                        let value = schema.resolve_or_err(&field.value)?;
                        let field =
                            ast::Field::new(&field.display_name, value.as_inline_type(schema)?);
                        Ok::<_, Error>(field)
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                result.push(ast::Struct::new(&s.type_name, fields));
            }
            Ty::Enum(e) => {
                for variant in &e.variants {
                    if let Some(ref value) = variant.value {
                        let value = schema.resolve_or_err(value)?;
                        let definitions = value.as_definitions(schema)?;
                        let renamed_definitions = prepend_context(definitions, &variant.name);
                        result.extend(renamed_definitions);
                    } else {
                        // Solidity does not support empty struct so we convert enum variants with no associated data into `{ bool _phantom }`
                        let field = ast::Field::new("_phantom", ast::Ty::Bool);
                        result.push(ast::Struct::new(&variant.name, [field]));
                    }
                }
            }
            Ty::Tuple(t) => {
                let fields: Vec<_> = t
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(idx, field)| {
                        let value = schema.resolve_or_err(&field.value)?;
                        let field =
                            ast::Field::new(format!("_{idx}"), value.as_inline_type(schema)?);
                        Ok::<_, Error>(field)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                result.push(ast::Struct::new("", fields));
            }
            _ => todo!(),
        };
        Ok(result)
    }
}

fn prepend_context(definitions: Vec<ast::Struct>, context: &str) -> Vec<ast::Struct> {
    definitions
        .into_iter()
        .map(|mut d| {
            if d.name == "" {
                d.name = format!("{context}");
            } else {
                d.name = format!("{context}_{}", d.name);
            }
            d
        })
        .collect()
}
