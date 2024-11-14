use crate::ty::{Enum, LinkingScheme, Struct, Tuple, Ty};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Container<L: LinkingScheme> {
    Struct(Struct<L>),
    Enum(Enum<L>),
    Tuple(Tuple<L>),
    Option {
        value: L::TypeLink,
    },
    Array {
        len: usize,
        value: L::TypeLink,
    },
    Vec {
        value: L::TypeLink,
    },
    Map {
        key: L::TypeLink,
        value: L::TypeLink,
    },
}

impl<L: LinkingScheme> Container<L> {
    /// Returns the number of child types required by this container.
    pub fn num_children(&self) -> usize {
        match self {
            Container::Struct(s) => s.fields.len(),
            Container::Enum(e) => e
                .variants
                .iter()
                .map(|variant| match &variant.value {
                    Some(_) => 1,
                    _ => 0,
                })
                .sum(),
            Container::Tuple(t) => t.fields.len(),
            Container::Option { .. } => 1,
            Container::Array { .. } => 1,
            Container::Vec { .. } => 1,
            Container::Map { .. } => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrNotAContainer;

impl<L: LinkingScheme> TryFrom<Ty<L>> for Container<L> {
    type Error = ErrNotAContainer;

    fn try_from(value: Ty<L>) -> Result<Self, Self::Error> {
        match value {
            Ty::Enum(e) => Ok(Container::Enum(e)),
            Ty::Struct(s) => Ok(Container::Struct(s)),
            Ty::Tuple(t) => Ok(Container::Tuple(t)),
            Ty::Option { value } => Ok(Container::Option { value }),
            Ty::Array { len, value } => Ok(Container::Array { len, value }),
            Ty::Map { key, value } => Ok(Container::Map { key, value }),
            Ty::Vec { value } => Ok(Container::Vec { value }),
            _ => Err(ErrNotAContainer),
        }
    }
}

impl<L: LinkingScheme> From<Container<L>> for Ty<L> {
    fn from(value: Container<L>) -> Self {
        match value {
            Container::Struct(s) => Ty::Struct(s),
            Container::Enum(e) => Ty::Enum(e),
            Container::Tuple(t) => Ty::Tuple(t),
            Container::Option { value } => Ty::Option { value },
            Container::Array { len, value } => Ty::Array { len, value },
            Container::Vec { value } => Ty::Vec { value },
            Container::Map { key, value } => Ty::Map { key, value },
        }
    }
}
