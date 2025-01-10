use std::str::FromStr;

use serde_json::{Number, Value};
use thiserror::Error;

use crate::schema::Primitive;
use crate::ty::byte_display::ByteParseError;
use crate::ty::visitor::{ResolutionError, TypeResolver, TypeVisitor};
use crate::ty::{Enum, IntegerType, LinkingScheme, Struct, Tuple, Ty};

pub type Result<T, E = EncodeError> = core::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("Core error: {0}")]
    Core(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(String),
    #[error(transparent)]
    ByteParsing(#[from] ByteParseError),
    #[error("Invalid discriminant `{discriminant}` for {type_name}")]
    InvalidDiscriminant {
        type_name: String,
        discriminant: String,
    },
    #[error("Expected {schema_type}, encountered invalid JSON value {value}")]
    InvalidType { schema_type: String, value: String },
    #[error("Invalid enum encoding: expected single variant, found object with {variants} JSON properties")]
    MalformedEnum { variants: usize },
    #[error(transparent)]
    UnresolvedType(#[from] ResolutionError),
    #[error("Expected type or field {name}, but it was not present")]
    MissingType { name: String },
    #[error("Expected an array of size {expected}, but only found {actual} elements in the JSON")]
    WrongArrayLength { expected: usize, actual: usize },
    #[error("Only array sizes that fit into u32 are supported; input contained size {0}")]
    InvalidVecLength(usize),
    #[error("The JSON contained an unexpected extra value: {value}")]
    UnusedInput { value: String },
}

pub struct Formatter<'a, W> {
    w: &'a mut W,
}

impl<'a, W> Formatter<'a, W> {
    pub fn new(w: &'a mut W) -> Self {
        Self { w }
    }
}

impl<'a, W: std::io::Write> std::io::Write for Formatter<'a, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.w.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.w.flush()
    }
}

pub struct EncodeVisitor<'fmt, W> {
    out: Formatter<'fmt, W>,
}

impl<'fmt, W> EncodeVisitor<'fmt, W> {
    pub fn new(f: &'fmt mut W) -> Result<Self, EncodeError> {
        Ok(Self {
            out: Formatter::new(f),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Context {
    value: serde_json::Value,
}

impl Context {
    pub fn new(input: &str) -> Result<Self, EncodeError> {
        Ok(Self {
            value: serde_json::from_str(input).map_err(|e| EncodeError::Json(e.to_string()))?,
        })
    }

    pub fn from_val(value: Value) -> Self {
        Self { value }
    }
}

macro_rules! serialize_primitive {
    ($self:ident, $val:expr, $as_fn:ident, $expected_type:literal, $downcast_to:ty) => {
        serialize_primitive!($self, $val, $as_fn, $expected_type, |v| {
            <$downcast_to>::try_from(v).ok()
        })
    };
    ($self:ident, $val:expr, $as_fn:ident, $expected_type:literal, $map_expr:expr) => {{
        let value = $val
            .$as_fn()
            .map($map_expr)
            .flatten()
            .ok_or(EncodeError::InvalidType {
                schema_type: $expected_type.to_string(),
                value: $val.to_string(),
            })?;
        borsh::to_writer(&mut $self.out, &value)?;
        Ok(()) as Self::ReturnType
    }};
    ($self:ident, $val:expr, $as_fn:ident, $expected_type:literal) => {
        serialize_primitive!($self, $val, $as_fn, $expected_type, |v| Some(v))
    };
}

impl<'fmt, W: std::io::Write, L: LinkingScheme> TypeVisitor<L> for EncodeVisitor<'fmt, W> {
    type Arg = Context;
    type ReturnType = Result<(), EncodeError>;
    fn visit_enum(
        &mut self,
        e: &Enum<L>,
        schema: &impl TypeResolver<LinkingScheme = L>,
        mut context: Context,
    ) -> Self::ReturnType {
        let (discriminant, inner_value) = match context.value {
            Value::String(s) => (s, None),
            Value::Object(o) => {
                if o.len() != 1 {
                    return Err(EncodeError::MalformedEnum { variants: o.len() });
                };
                let (d, i) = o.into_iter().next().unwrap();
                (d, Some(i))
            }
            _ => {
                return Err(EncodeError::InvalidType {
                    schema_type: format!("enum {}", e.type_name),
                    value: context.value.to_string(),
                })
            }
        };

        let (index, variant) = e
            .variants
            .iter()
            .enumerate()
            .find(|(_, v)| v.serde_name == discriminant)
            .ok_or(EncodeError::InvalidDiscriminant {
                type_name: e.type_name.clone(),
                discriminant: discriminant.to_owned(),
            })?;
        borsh::to_writer(&mut self.out, &(index as u8))?;

        if let Some(maybe_resolved) = &variant.value {
            let inner_type = schema.resolve_or_err(maybe_resolved)?;
            let Some(inner_value) = inner_value else {
                return Err(EncodeError::MissingType {
                    name: format!("{}.{} data", e.type_name, variant.name),
                });
            };
            context.value = inner_value;
            inner_type.visit(schema, self, context)?;
        } else if let Some(extra_value) = inner_value {
            return Err(EncodeError::UnusedInput {
                value: extra_value.to_string(),
            });
        }
        Ok(())
    }
    fn visit_struct(
        &mut self,
        s: &Struct<L>,
        schema: &impl TypeResolver<LinkingScheme = L>,
        mut context: Context,
    ) -> Self::ReturnType {
        let mut json_fields = match context.value {
            Value::Object(o) => o,
            _ => {
                return Err(EncodeError::InvalidType {
                    schema_type: format!("{} struct", s.type_name),
                    value: context.value.to_string(),
                })
            }
        };

        for field in &s.fields {
            // TODO: ensure skip is handled correctly
            let json_value =
                json_fields
                    .remove(&field.serde_display_name)
                    .ok_or(EncodeError::MissingType {
                        name: format!("{}.{}", s.type_name, field.display_name),
                    })?;
            let inner_type = schema.resolve_or_err(&field.value)?;
            context.value = json_value;
            // TODO: adjust `Context` so it can return references to views over the full JSON,
            // without needing to clone. This is slightly annoying to ensure lifetimes are
            // correctly managed. Easiest solution is likely using JSON paths using value.pointer()
            inner_type.visit(schema, self, context.clone())?;
        }
        if !json_fields.is_empty() {
            return Err(EncodeError::UnusedInput {
                value: Value::Object(json_fields).to_string(),
            });
        }
        Ok(())
    }

    fn visit_tuple(
        &mut self,
        t: &Tuple<L>,
        schema: &impl TypeResolver<LinkingScheme = L>,
        context: Context,
    ) -> Self::ReturnType {
        if t.fields.len() == 1 {
            // Trivial tuples aren't wrapped in JSON; forward the value directly to the inner field
            let inner_type = schema.resolve_or_err(&t.fields.first().unwrap().value)?;
            inner_type.visit(schema, self, context)
        } else {
            // iterate array, visit each type
            let arr = context.value.as_array().ok_or(EncodeError::InvalidType {
                schema_type: "array".to_string(),
                value: context.value.to_string(),
            })?;
            if arr.len() != t.fields.len() {
                return Err(EncodeError::WrongArrayLength {
                    expected: t.fields.len(),
                    actual: arr.len(),
                });
            }
            for (field, val) in t.fields.iter().zip(arr) {
                let inner_type = schema.resolve_or_err(&field.value)?;
                inner_type.visit(schema, self, Context::from_val(val.clone()))?;
            }
            Ok(())
        }
    }

    fn visit_option(
        &mut self,
        value: &L::TypeLink,
        schema: &impl TypeResolver<LinkingScheme = L>,
        context: Context,
    ) -> Self::ReturnType {
        match context.value {
            Value::Null => {
                borsh::to_writer(&mut self.out, &0u8)?;
            }
            _ => {
                borsh::to_writer(&mut self.out, &1u8)?;
                schema.resolve_or_err(value)?.visit(schema, self, context)?;
            }
        }

        Ok(())
    }

    fn visit_primitive(
        &mut self,
        p: crate::schema::Primitive,
        _schema: &impl TypeResolver<LinkingScheme = L>,
        context: Context,
    ) -> Self::ReturnType {
        match p {
            Primitive::Float32 => {
                serialize_primitive!(self, context.value, as_f64, "f32", |f| {
                    let f = f as f32;
                    if f.is_finite() {
                        Some(f)
                    } else {
                        None
                    }
                })
            }
            Primitive::Float64 => serialize_primitive!(self, context.value, as_f64, "f64"),
            Primitive::Boolean => serialize_primitive!(self, context.value, as_bool, "bool"),
            Primitive::Integer(int, _) => match int {
                IntegerType::i8 => serialize_primitive!(self, context.value, as_i64, "i8", i8),
                IntegerType::i16 => serialize_primitive!(self, context.value, as_i64, "i16", i16),
                IntegerType::i32 => serialize_primitive!(self, context.value, as_i64, "i32", i32),
                IntegerType::i64 => serialize_primitive!(self, context.value, as_i64, "i64"),
                IntegerType::i128 => {
                    serialize_primitive!(self, context.value, as_i64, "i128", i128)
                }
                IntegerType::u8 => serialize_primitive!(self, context.value, as_u64, "u8", u8),
                IntegerType::u16 => serialize_primitive!(self, context.value, as_u64, "u16", u16),
                IntegerType::u32 => serialize_primitive!(self, context.value, as_u64, "u32", u32),
                IntegerType::u64 => serialize_primitive!(self, context.value, as_u64, "u64", u64),
                IntegerType::u128 => {
                    serialize_primitive!(self, context.value, as_u64, "u128", u128)
                }
            },
            Primitive::ByteArray { len, display } => {
                let verify_len = |actual: usize| {
                    if actual != len {
                        Err(EncodeError::WrongArrayLength {
                            expected: len,
                            actual,
                        })
                    } else {
                        Ok(())
                    }
                };
                match context.value {
                    Value::Array(arr) => {
                        verify_len(arr.len())?;
                        for byte in arr {
                            serialize_primitive!(self, byte.clone(), as_u64, "byte", u8)?;
                        }
                    }
                    Value::String(str) => {
                        let arr = display.parse(&str)?;
                        verify_len(arr.len())?;
                        for byte in arr {
                            borsh::to_writer(&mut self.out, &byte)?;
                        }
                    }
                    _ => {
                        return Err(EncodeError::InvalidType {
                            schema_type: "byte array".to_string(),
                            value: context.value.to_string(),
                        })
                    }
                };
                Ok(())
            }
            Primitive::ByteVec { display } => {
                let vec = match context.value {
                    Value::Array(vec) => vec
                        .iter()
                        .map(|v| {
                            v.as_u64().and_then(|u| u8::try_from(u).ok()).ok_or(
                                EncodeError::InvalidType {
                                    schema_type: "byte".to_string(),
                                    value: v.to_string(),
                                },
                            )
                        })
                        .collect::<Result<Vec<u8>, _>>()?,
                    Value::String(str) => display.parse(&str)?,
                    _ => {
                        return Err(EncodeError::InvalidType {
                            schema_type: "byte vector".to_string(),
                            value: context.value.to_string(),
                        })
                    }
                };
                borsh::to_writer(&mut self.out, &vec)?;
                Ok(())
            }
            Primitive::String => serialize_primitive!(self, context.value, as_str, "string"),
            Primitive::Skip { .. } => {
                // TODO: is this always correct?
                Ok(())
            }
        }
    }

    fn visit_array(
        &mut self,
        len: &usize,
        value: &L::TypeLink,
        schema: &impl TypeResolver<LinkingScheme = L>,
        context: Context,
    ) -> Self::ReturnType {
        let arr = context.value.as_array().ok_or(EncodeError::InvalidType {
            schema_type: "array".to_string(),
            value: context.value.to_string(),
        })?;
        if arr.len() != *len {
            return Err(EncodeError::WrongArrayLength {
                expected: *len,
                actual: arr.len(),
            });
        }
        let inner_type = schema.resolve_or_err(value)?;
        for val in arr.iter() {
            inner_type.visit(schema, self, Context::from_val(val.clone()))?;
        }
        Ok(())
    }

    fn visit_vec(
        &mut self,
        value: &L::TypeLink,
        schema: &impl TypeResolver<LinkingScheme = L>,
        context: Context,
    ) -> Self::ReturnType {
        let vec = context.value.as_array().ok_or(EncodeError::InvalidType {
            schema_type: "vector".to_string(),
            value: context.value.to_string(),
        })?;
        let len = u32::try_from(vec.len()).map_err(|_| EncodeError::InvalidVecLength(vec.len()))?;
        borsh::to_writer(&mut self.out, &len)?;
        let inner_type = schema.resolve_or_err(value)?;
        for val in vec.iter() {
            inner_type.visit(schema, self, Context::from_val(val.clone()))?;
        }
        Ok(())
    }

    fn visit_map(
        &mut self,
        key: &L::TypeLink,
        value: &L::TypeLink,
        schema: &impl TypeResolver<LinkingScheme = L>,
        context: Context,
    ) -> Self::ReturnType {
        let map = context.value.as_object().ok_or(EncodeError::InvalidType {
            schema_type: "map".to_string(),
            value: context.value.to_string(),
        })?;
        let len = u32::try_from(map.len()).map_err(|_| EncodeError::InvalidVecLength(map.len()))?;
        borsh::to_writer(&mut self.out, &len)?;
        let key_type = schema.resolve_or_err(key)?;
        let value_type = schema.resolve_or_err(value)?;
        for val in map.iter() {
            // JSON coerces all map keys to string. This makes some complex Rust types invalid for
            // JSON serialization as a map key.
            // But notably, number types are valid but still show up as a JSON string.
            let key_value = if matches!(key_type, Ty::Integer(_, _) | Ty::Float32 | Ty::Float64) {
                Value::Number(
                    Number::from_str(&val.0.clone())
                        .map_err(|e| EncodeError::Json(e.to_string()))?,
                )
            } else {
                Value::String(val.0.clone())
            };
            key_type.visit(schema, self, Context::from_val(key_value))?;
            value_type.visit(schema, self, Context::from_val(val.1.clone()))?;
        }
        Ok(())
    }
}
