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
    pub fn as_inline_type(&self) -> ast::Ty {
        match self {
            Ty::Integer(_, _) => ast::Ty::Uint256,
            Ty::Struct(s) => ast::Ty::Ident(s.type_name.clone()),
            _ => todo!(),
        }
    }

    pub fn as_definition(&self, schema: &Schema) -> Result<ast::Item, Error> {
        match self {
            Ty::Struct(s) => {
                let fields: Vec<_> = s
                    .fields
                    .iter()
                    .map(|field| {
                        let value = schema.resolve_or_err(&field.value)?;
                        Ok::<_, Error>(ast::Field {
                            name: field.display_name.clone(),
                            ty: value.as_inline_type(),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(ast::Item::Struct {
                    name: s.type_name.clone(),
                    fields,
                })
            }
            _ => todo!(),
        }
    }
}
