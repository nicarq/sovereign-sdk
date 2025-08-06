use std::borrow::Cow;

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

impl Ty<IndexLinking> {
    pub fn as_inline_type(&self) -> Cow<'_, str> {
        match self {
            Ty::Struct(s) => Cow::Borrowed(&s.type_name),
            Ty::Boolean => Cow::Borrowed("bool"),
            Ty::Integer(_, _) => Cow::Borrowed("uint256"),
            Ty::String => Cow::Borrowed("string"),
            Ty::ByteVec { .. } => Cow::Borrowed("bytes"),
            Ty::Enum(_) => todo!(),
            Ty::Tuple(_) => todo!(),
            Ty::Option { .. } => todo!(),
            Ty::ByteArray { .. } => todo!(),
            Ty::Float32 => todo!(),
            Ty::Float64 => todo!(),
            Ty::Skip { .. } => todo!(),
            Ty::Array { .. } => todo!(),
            Ty::Vec { .. } => todo!(),
            Ty::Map { .. } => todo!(),
        }
    }

    pub fn as_definition<'t>(&'t self, schema: &Schema) -> Result<Cow<'t, str>, Error> {
        match self {
            Ty::Struct(s) => {
                let mut out = String::new();
                out.push_str(&format!("    struct {} {{\n", s.type_name));

                for field in &s.fields {
                    let value = schema.resolve_or_err(&field.value)?;
                    let field_definition = format!(
                        "        {} {};\n",
                        value.as_inline_type(),
                        field.display_name
                    );
                    out.push_str(&field_definition);
                }

                out.push_str("    }\n");
                Ok(Cow::Owned(out))
            }
            _ => todo!(),
        }
    }
}
