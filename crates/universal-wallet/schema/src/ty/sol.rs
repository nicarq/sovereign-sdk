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
            _ => todo!(),
        };
        Ok(ty)
    }

    pub fn as_definition(&self, schema: &Schema) -> Result<Vec<ast::Struct>, Error> {
        let mut result = vec![];
        match self {
            Ty::Struct(s) => {
                let fields: Vec<_> = s
                    .fields
                    .iter()
                    .map(|field| {
                        let value = schema.resolve_or_err(&field.value)?;
                        Ok::<_, Error>(ast::Field {
                            name: field.display_name.clone(),
                            ty: value.as_inline_type(schema)?,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                result.push(ast::Struct {
                    name: s.type_name.clone(),
                    fields,
                });
            }
            Ty::Enum(e) => {
                for variant in &e.variants {
                    if let Some(ref value) = variant.value {
                        let value = schema.resolve_or_err(value)?;
                        let definitions = value.as_definition(schema)?;
                        result.extend(definitions);
                    } else {
                        // Solidity does not support empty struct so we convert enum variants with no associated data into `{ bool _phantom }`
                        result.push(ast::Struct {
                            name: format!("__SovVirtualWallet_CallMessage_{}", variant.name),
                            fields: vec![ast::Field {
                                name: "_phantom".into(),
                                ty: ast::Ty::Bool,
                            }],
                        });
                    }
                }
            }
            _ => (),
        };
        Ok(result)
    }
}
