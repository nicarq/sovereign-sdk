use std::fmt::Write;

use thiserror::Error;

use crate::schema::Primitive;
use crate::ty::visitor::{ResolutionError, TypeResolver, TypeVisitor};
use crate::ty::{ByteDisplay, Enum, IntegerDisplay, IntegerType, LinkingScheme, Struct, Tuple};

type Delimiters = (&'static str, &'static str);

pub type Result<T, E = FormatError> = core::result::Result<T, E>;

/// The largest input size to display (in bytes)
pub const MAX_INPUT_CHUNK: usize = 65;

#[derive(Debug, Error, Clone)]
pub enum FormatError {
    #[error("Core error: {0}")]
    Core(#[from] core::fmt::Error),
    #[error("The input could not be displayed as bech32: {0}")]
    InvalidBech32(#[from] bech32::EncodeError),
    #[error("The input is not a valid utf-8 string: {0}")]
    InvalidString(#[from] core::str::Utf8Error),
    #[error("Invalid discriminant `{discriminant}` for {type_name}")]
    InvalidDiscriminant { type_name: String, discriminant: u8 },
    #[error(transparent)]
    UnresolvedType(#[from] ResolutionError),
    #[error("A discriminant is required for items of type `{type_name}` but the input ended without providing one.")]
    MissingDiscriminant { type_name: String },
    #[error("The input claimed to provide an integer {claimed_size} bytes wide, but the maximum allowed size is 16 bytes.")]
    IntegerTooLarge { claimed_size: u8 },
    #[error("The input claimed to provide an integer {claimed_size} bytes wide, but only provided {bytes_available} additional bytes of input.")]
    MissingIntegerInput {
        claimed_size: u8,
        bytes_available: u8,
    },
    #[error("The input claimed to provide a byte array {claimed_size} bytes wide, but only provided {bytes_available} additional bytes of input.")]
    MissingBytesInput {
        claimed_size: usize,
        bytes_available: usize,
    },
    #[error("The input should have contained a vector but did not provide one.")]
    MissingVecLength,

    #[error("The input should have contained a string but did not provide one.")]
    MissingStringLength,
    #[error("The provided input had leftover bytes that weren't displayed.")]
    UnusedInput,
    #[error(
        "A structs's display template did not provide sufficient slots to display its fields."
    )]
    InsufficientTemplateSlots,
    #[error("A struct's display template had more slots than the struct had fields.")]
    UnusedTemplateSlots,
}

pub struct Silencer<'a, W> {
    f: &'a mut W,
    silent: bool,
}

impl<'a, W> Silencer<'a, W> {
    pub fn new(f: &'a mut W) -> Self {
        Self { f, silent: false }
    }
}

impl<'a, W: core::fmt::Write> core::fmt::Write for Silencer<'a, W> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if self.silent {
            Ok(())
        } else {
            self.f.write_str(s)
        }
    }
}

pub struct DisplayVisitor<'a, 'fmt, W> {
    input: &'a mut &'a [u8],
    f: Silencer<'fmt, W>,
}

impl<'a, 'fmt, W> DisplayVisitor<'a, 'fmt, W> {
    pub fn has_displayed_whole_input(&self) -> bool {
        self.input.is_empty()
    }

    pub fn remaining_input(&self) -> &[u8] {
        self.input
    }
}

/// In case an enum variant contains nested trivial tuples, propagate the virtual-ness transitively
/// to the actual content
fn tuple_is_enum_contents(context: &Context) -> bool {
    context.parent_type == ParentType::Enum
        || context.parent_type == ParentType::Tuple(IsVirtual::Yes, IsTrivial::Yes)
}

// To avoid unnecessary nested brackets, we apply an optimization to the display of tuples where
// tuples with a single variant do not wrap their contents in parentheses by default.
// This creates a special case when a tuple-variant of an enum has a single field. In that case,
// we need to "make up" the removed delimiter later on in the rendering process.
//
// The "make-up" delimiters are as follows:
// - Inner enum: "."
// - Inner tuple: N/A
// - String-like item: Wrap in parentheses
// - Other: Add an extra " " after the usual delimiter
impl<'a, 'fmt, W> DisplayVisitor<'a, 'fmt, W> {
    pub fn new(input: &'a mut &'a [u8], f: &'fmt mut W) -> Self {
        Self {
            input,
            f: Silencer::new(f),
        }
    }
    fn tuple_delimiters<L: LinkingScheme>(
        &self,
        tuple: &Tuple<L>,
        context: &Context,
        schema: &impl TypeResolver<LinkingScheme = L>,
    ) -> Delimiters {
        let first_child_is_primitive = schema
            .resolve_or_err(&tuple.fields[0].value)
            .map_or(false, |v| v.is_primitive());

        if tuple.fields.len() == 1 && !(tuple_is_enum_contents(context) && first_child_is_primitive)
        {
            ("", "")
        } else {
            ("(", ")")
        }
    }

    fn enum_delimiters<L: LinkingScheme>(&mut self, _e: &Enum<L>, context: &Context) -> Delimiters {
        match context.parent_type {
            ParentType::Tuple(IsVirtual::Yes, IsTrivial::Yes)
            | ParentType::Enum
            | ParentType::Vec
            | ParentType::Map => (".", ""),
            ParentType::Tuple(_, _) | ParentType::Struct(_) | ParentType::None => ("", ""),
        }
    }

    fn struct_delimiters<L: LinkingScheme>(&self, s: &Struct<L>, context: &Context) -> Delimiters {
        if s.fields.is_empty() {
            return ("", "");
        }
        match context.parent_type {
            ParentType::Tuple(IsVirtual::Yes, _) => (" { ", " }"),
            ParentType::Struct(_)
            | ParentType::None
            | ParentType::Tuple(_, _)
            | ParentType::Vec
            | ParentType::Map => ("{ ", " }"),
            ParentType::Enum => (" { ", " }"),
        }
    }

    /// Generates a template/format string to display a struct if none was provided as an
    /// attribute. The default template either displays the typename if the struct has no
    /// fields, or looks like this (for a hypothetical struct with three fields named as below):
    /// ```text
    /// "{ field_one: {}, field_two: {}, field_three: {} }"
    /// ```
    /// In other words, simply lists out the field names followed by an unnamed substitution (that
    /// will be filled in with the field's values, in order), enclosed in braces and
    /// comma-separated.
    fn struct_default_template<L: LinkingScheme>(
        &self,
        s: &Struct<L>,
        context: &Context,
        schema: &impl TypeResolver<LinkingScheme = L>,
    ) -> String {
        let mut template = String::new();
        let (opener, closer) = self.struct_delimiters(s, context);
        template.push_str(opener);
        if s.fields.is_empty() {
            template.push_str(&s.type_name);
        } else {
            for (i, field) in s.fields.iter().enumerate() {
                if field.silent {
                    continue;
                }
                if schema
                    .resolve_or_err(&field.value)
                    .map_or(false, |inner| inner.is_skip())
                {
                    continue;
                }
                if i > 0 {
                    template.push_str(self.item_separator());
                }
                template.push_str(&field.display_name);
                template.push_str(": {}");
            }
        }
        template.push_str(closer);
        template
    }

    /// Generates a template/format string to display a tuple if none was provided as an
    /// attribute. The default template looks like this, for example for a tuple with three values:
    /// ```text
    /// "({}, {}, {})"
    /// ```
    /// In other words, simply lists out the fields as unnamed substitutions (which will be filled in
    /// with the tuple's values in order, during display), enclosed in brackets and comma-separated.
    /// The brackets are sometimes omitted - see the implementation of `tuple_delimiters` for the
    /// logic.
    fn tuple_default_template<L: LinkingScheme>(
        &self,
        t: &Tuple<L>,
        context: &Context,
        schema: &impl TypeResolver<LinkingScheme = L>,
    ) -> String {
        let mut template = String::new();
        let (opener, closer) = self.tuple_delimiters(t, context, schema);
        template.push_str(opener);
        for (i, field) in t.fields.iter().enumerate() {
            if field.silent {
                continue;
            }
            if i > 0 {
                template.push_str(self.item_separator());
            }
            template.push_str("{}");
        }
        template.push_str(closer);
        template
    }

    fn map_delimiters(&mut self, context: &Context) -> Delimiters {
        match context.parent_type {
            ParentType::Tuple(IsVirtual::Yes, _) => (" { ", " }"),
            ParentType::Struct(_)
            | ParentType::None
            | ParentType::Tuple(_, _)
            | ParentType::Vec
            | ParentType::Enum
            | ParentType::Map => ("{ ", " }"),
        }
    }

    fn vec_delimiters(&mut self, context: &Context) -> Delimiters {
        match context.parent_type {
            ParentType::Tuple(IsVirtual::Yes, _) => (" [", "]"),
            ParentType::None
            | ParentType::Struct(_)
            | ParentType::Tuple(_, _)
            | ParentType::Vec
            | ParentType::Enum
            | ParentType::Map => ("[", "]"),
        }
    }

    fn item_separator(&self) -> &'static str {
        ", "
    }
}

impl<'a, 'fmt, W: Write> DisplayVisitor<'a, 'fmt, W> {
    pub fn read_usize_borsh(&mut self) -> Result<usize, FormatError> {
        if self.input.len() < 4 {
            return Err(FormatError::MissingIntegerInput {
                claimed_size: 4,
                bytes_available: self.input.len() as u8,
            });
        }
        let len = u32::from_le_bytes(
            self.input[..4]
                .try_into()
                .expect("Converting [u8;4] to u32 is infallible"),
        ) as usize;
        *self.input = &self.input[4..];
        Ok(len)
    }

    fn check_remaining_bytes(&self, len: usize) -> Result<(), FormatError> {
        if self.input.len() < len {
            return Err(FormatError::MissingBytesInput {
                claimed_size: len,
                bytes_available: self.input.len(),
            });
        }
        Ok(())
    }

    /// Splits the first `len` bytes from the input, returning them as a slice and updating the input buffer.
    /// Returns an error if there are not enough bytes remaining to fulfill the request.
    pub fn advance(&mut self, len: usize) -> Result<&[u8], FormatError> {
        self.check_remaining_bytes(len)?;
        let (leading, rest) = self.input.split_at(len);
        *self.input = rest;
        Ok(leading)
    }

    pub fn display_byte_sequence(
        &mut self,
        len: usize,
        display: ByteDisplay,
        _context: Context,
    ) -> Result<(), FormatError> {
        self.check_remaining_bytes(len)?;

        if len > MAX_INPUT_CHUNK {
            display.format(&self.input[..MAX_INPUT_CHUNK], &mut self.f)?;
            self.f.write_fmt(format_args!(
                " (trailing {} bytes truncated)",
                len - MAX_INPUT_CHUNK
            ))?;
        } else {
            display.format(&self.input[..len], &mut self.f)?;
        }
        *self.input = &self.input[len..];
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Context {
    parent_type: ParentType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsVirtual {
    Yes,
    No,
}

/// Tuple wrapping a single value
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsTrivial {
    Yes,
    No,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentType {
    None,
    Struct(IsVirtual),
    Tuple(IsVirtual, IsTrivial),
    Enum,
    Vec,
    Map,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            parent_type: ParentType::None,
        }
    }
}

macro_rules! display_int {
    ($t:ty, $input:expr, $disp:expr, $f:expr) => {{
        if $input.len() < core::mem::size_of::<$t>() {
            return Err(FormatError::MissingIntegerInput {
                claimed_size: core::mem::size_of::<$t>() as u8,
                bytes_available: $input.len() as u8,
            });
        }
        let buf = &$input[..core::mem::size_of::<$t>()];
        *$input = &$input[core::mem::size_of::<$t>()..];
        match $disp {
            IntegerDisplay::Hex => {
                write!($f, "{:#x}", <$t>::from_le_bytes(buf.try_into().unwrap()))?
            }
            IntegerDisplay::Decimal => {
                write!($f, "{}", <$t>::from_le_bytes(buf.try_into().unwrap()))?
            }
        }
        Ok(())
    }};
}

impl<'a, 'fmt, W: Write, L: LinkingScheme> TypeVisitor<L> for DisplayVisitor<'a, 'fmt, W> {
    type Arg = Context;
    type ReturnType = Result<(), FormatError>;
    fn visit_enum(
        &mut self,
        e: &Enum<L>,
        schema: &impl TypeResolver<LinkingScheme = L>,
        mut context: Context,
    ) -> Self::ReturnType {
        if self.input.is_empty() && !e.variants.is_empty() {
            return Err(FormatError::MissingDiscriminant {
                type_name: e.type_name.clone(),
            });
        }

        // If the enum is displayed as part of a tuple, the user doesn't have any context about what the type is,
        // since there's no field name. In that case we display the full type name.
        if matches!(context.parent_type, ParentType::Tuple(IsVirtual::No, _))
            || context.parent_type == ParentType::Vec
        {
            self.f.write_str(&e.type_name)?;
        }

        let (open, close) = self.enum_delimiters(e, &context);
        self.f.write_str(open)?;

        let variant =
            e.variants
                .get(self.input[0] as usize)
                .ok_or(FormatError::InvalidDiscriminant {
                    type_name: e.type_name.clone(),
                    discriminant: self.input[0],
                })?;
        *self.input = &self.input[1..];
        if variant.template.is_none() {
            write!(self.f, "{}", variant.name)?;
        }
        context.parent_type = ParentType::Enum;
        if let Some(maybe_resolved) = &variant.value {
            let inner = schema.resolve_or_err(maybe_resolved)?;
            inner.visit(schema, self, context)?;
        }
        self.f.write_str(close)?;
        Ok(())
    }
    fn visit_struct(
        &mut self,
        s: &Struct<L>,
        schema: &impl TypeResolver<LinkingScheme = L>,
        mut context: Context,
    ) -> Self::ReturnType {
        let template = s
            .template
            .clone()
            .unwrap_or_else(|| self.struct_default_template(s, &context, schema));
        let mut template = template.as_str();

        context.parent_type = ParentType::Struct(IsVirtual::No);
        for field in s.fields.iter() {
            // Save the previous state of the silent flag so we can restore it after displaying the field.
            let was_silent = self.f.silent;
            if field.silent {
                self.f.silent = true;
            }

            let inner_ty = schema.resolve_or_err(&field.value)?;
            if !field.silent && !inner_ty.is_skip() {
                let Some((before_next_field, rest)) = template.split_once("{}") else {
                    return Err(FormatError::InsufficientTemplateSlots);
                };
                self.f.write_str(before_next_field)?;
                template = rest;
            }

            inner_ty.visit(schema, self, context)?;
            // Restore the silent flag to its previous state
            self.f.silent = was_silent;
        }
        if template.contains("{}") {
            return Err(FormatError::UnusedTemplateSlots);
        }
        self.f.write_str(template)?;
        Ok(())
    }

    fn visit_tuple(
        &mut self,
        t: &Tuple<L>,
        schema: &impl TypeResolver<LinkingScheme = L>,
        mut context: Context,
    ) -> Self::ReturnType {
        let template = t
            .template
            .clone()
            .unwrap_or_else(|| self.tuple_default_template(t, &context, schema));
        let mut template = template.as_str();

        let is_virtual = if tuple_is_enum_contents(&context) {
            IsVirtual::Yes
        } else {
            IsVirtual::No
        };
        let trivial = if t.fields.len() == 1 {
            IsTrivial::Yes
        } else {
            IsTrivial::No
        };
        context.parent_type = ParentType::Tuple(is_virtual, trivial);

        for field in t.fields.iter() {
            // Save the previous state of the silent flag so we can restore it after displaying the field.
            let was_silent = self.f.silent;
            if field.silent {
                self.f.silent = true;
            }
            if !field.silent {
                let Some((before_next_field, rest)) = template.split_once("{}") else {
                    return Err(FormatError::InsufficientTemplateSlots);
                };
                self.f.write_str(before_next_field)?;
                template = rest;
            }

            schema
                .resolve_or_err(&field.value)?
                .visit(schema, self, context)?;
            // Restore the silent flag to its previous state
            self.f.silent = was_silent;
        }
        if template.contains("{}") {
            return Err(FormatError::UnusedTemplateSlots);
        }
        self.f.write_str(template)?;
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
                let value = self.advance(4)?;
                let value = f32::from_le_bytes(value.try_into().unwrap());
                write!(self.f, "{}", value)?;
                Ok(())
            }
            Primitive::Float64 => {
                let value = self.advance(8)?;
                let value = f64::from_le_bytes(value.try_into().unwrap());
                write!(self.f, "{}", value)?;
                Ok(())
            }
            Primitive::Boolean => {
                let value = self.advance(1)?;
                match value[0] {
                    0 => self.f.write_str("false")?,
                    1 => self.f.write_str("true")?,
                    _ => {
                        return Err(FormatError::InvalidDiscriminant {
                            type_name: "bool".to_string(),
                            discriminant: value[0],
                        });
                    }
                }
                Ok(())
            }
            Primitive::Integer(int, display) => match int {
                IntegerType::i8 => display_int!(i8, self.input, display, self.f),
                IntegerType::i16 => display_int!(i16, self.input, display, self.f),
                IntegerType::i32 => display_int!(i32, self.input, display, self.f),
                IntegerType::i64 => display_int!(i64, self.input, display, self.f),
                IntegerType::i128 => display_int!(i128, self.input, display, self.f),
                IntegerType::u8 => display_int!(u8, self.input, display, self.f),
                IntegerType::u16 => display_int!(u16, self.input, display, self.f),
                IntegerType::u32 => display_int!(u32, self.input, display, self.f),
                IntegerType::u64 => display_int!(u64, self.input, display, self.f),
                IntegerType::u128 => display_int!(u128, self.input, display, self.f),
            },
            Primitive::ByteArray { len, display } => {
                self.display_byte_sequence(len, display, context)
            }
            Primitive::ByteVec { display } => {
                let len = self
                    .read_usize_borsh()
                    .or(Err(FormatError::MissingVecLength))?;
                self.display_byte_sequence(len, display, context)
            }
            Primitive::String => {
                let len = self
                    .read_usize_borsh()
                    .or(Err(FormatError::MissingStringLength))?;
                // This is a copy-paste of the `advance` function. We have to do this because
                // the borrow checker isn't smart enough to know that a call to advance is safe here.
                let content = {
                    self.check_remaining_bytes(len)?;
                    let (leading, rest) = self.input.split_at(len);
                    *self.input = rest;
                    leading
                };
                let content = std::str::from_utf8(content)?;
                write!(self.f, "\"{}\"", content)?;
                Ok(())
            }
            Primitive::Skip { len } => {
                self.advance(len)?;
                Ok(())
            }
        }
    }

    fn visit_array(
        &mut self,
        len: &usize,
        value: &L::TypeLink,
        schema: &impl TypeResolver<LinkingScheme = L>,
        mut context: Context,
    ) -> Self::ReturnType {
        let inner = schema.resolve_or_err(value)?;
        let (open, close) = self.vec_delimiters(&context);
        self.f.write_str(open)?;
        context.parent_type = ParentType::Vec;
        for i in 0..*len {
            if i > 0 {
                self.f.write_str(", ")?;
            }
            inner.visit(schema, self, context)?;
        }
        self.f.write_str(close)?;
        Ok(())
    }

    fn visit_vec(
        &mut self,
        value: &L::TypeLink,
        schema: &impl TypeResolver<LinkingScheme = L>,
        mut context: Context,
    ) -> Self::ReturnType {
        let len = self.read_usize_borsh()?;
        let inner = schema.resolve_or_err(value)?;
        let (open, close) = self.vec_delimiters(&context);
        self.f.write_str(open)?;
        context.parent_type = ParentType::Vec;
        for i in 0..len {
            if i > 0 {
                self.f.write_str(", ")?;
            }
            inner.visit(schema, self, context)?;
        }
        self.f.write_str(close)?;
        Ok(())
    }

    fn visit_map(
        &mut self,
        key: &L::TypeLink,
        value: &L::TypeLink,
        schema: &impl TypeResolver<LinkingScheme = L>,
        mut context: Context,
    ) -> Self::ReturnType {
        let len = self.read_usize_borsh()?;
        let key = schema.resolve_or_err(key)?;
        let value = schema.resolve_or_err(value)?;
        let (open, close) = self.map_delimiters(&context);
        self.f.write_str(open)?;
        context.parent_type = ParentType::Map;
        for i in 0..len {
            if i > 0 {
                self.f.write_str(", ")?;
            }
            key.visit(schema, self, context)?;
            self.f.write_str(": ")?;
            value.visit(schema, self, context)?;
        }
        self.f.write_str(close)?;
        Ok(())
    }
}
