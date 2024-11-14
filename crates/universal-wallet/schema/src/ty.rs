pub mod visitor;
use core::panic;
use std::fmt::Debug;

use bech32::{Bech32, Bech32m, Hrp};
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "serde")]
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::display::FormatError;
use crate::schema::{IndexLinking, Link, OverrideSchema, Primitive};

pub trait LinkingScheme: Clone + Debug {
    /// The type used to link to other types in the schema representation. Usually, this is an enum
    /// which represents primitives with an immediate value and complex types with some kind of pointer.
    #[cfg(not(feature = "serde"))]
    type TypeLink: Clone + Debug + PartialEq + Eq + BorshSerialize + BorshDeserialize;
    #[cfg(feature = "serde")]
    type TypeLink: Clone
        + Debug
        + PartialEq
        + Eq
        + BorshSerialize
        + BorshDeserialize
        + Serialize
        + DeserializeOwned;
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Ty<L: LinkingScheme> {
    Enum(Enum<L>),
    Struct(Struct<L>),
    Tuple(Tuple<L>),
    Option {
        value: L::TypeLink,
    },
    Integer(IntegerType, IntegerDisplay),
    ByteArray {
        len: usize,
        display: ByteDisplay,
    },
    Float32,
    Float64,
    String,
    Boolean,
    Skip {
        len: usize,
    },
    ByteVec {
        display: ByteDisplay,
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

impl<L: LinkingScheme> Ty<L> {
    /// Returns true if the type is a skip type
    pub fn is_skip(&self) -> bool {
        matches!(self, Ty::Skip { .. })
    }
}

impl Ty<IndexLinking> {
    /// Fills the next available placeholder in the type with the given link, panicking if no placeholder is available.
    pub fn fill_next_placholder(&mut self, child: Link) {
        let err_msg = format!(
            "Called `fill_next_placholder` on a type with no placeholders: {:?}",
            self
        );
        match self {
            Ty::Enum(e) => {
                e.variants
                    .iter_mut()
                    .find(|v| v.value == Some(Link::Placeholder))
                    .expect(&err_msg).value = Some(child);
            }
            Ty::Struct(s) => {
                s.fields
                    .iter_mut()
                    .find(|field| field.value == Link::Placeholder)
                    .expect(&err_msg)
                    .value = child;
            }
            Ty::Tuple(t) => {
                t.fields
                    .iter_mut()
                    .find(|field| field.value == Link::Placeholder)
                    .expect(&err_msg)
                    .value = child;
            }
            Ty::Option { value } => if *value == Link::Placeholder {
                *value = child;
            } else {
                panic!("{}", err_msg);
            }
            Ty::Array { value, .. } => {
				if *value == Link::Placeholder {
					*value = child;
				} else {
					panic!("{}", err_msg);
				}
			}
            Ty::Vec { value } => if *value == Link::Placeholder {
				*value = child;
			} else {
				panic!("{}", err_msg);
			}
            Ty::Map { key, value } => if *key == Link::Placeholder {
				*key = child;
			} else if *value == Link::Placeholder {
				*value = child;
			} else {
				panic!("{}", err_msg);
			}
            _ => panic!(
                "Tried to fill a placholder on a type with no children. Only Vec, Tuple, Option, Array, Struct and Map types have children. Self: {:?}",
                self
            ),
        }
    }
}

impl<L: LinkingScheme> Ty<L> {
    pub fn is_primitive(&self) -> bool {
        // Match exhaustively in case additional primitives are added later
        match self {
            Ty::Enum(_)
            | Ty::Struct(_)
            | Ty::Tuple(_)
            | Ty::Option { .. }
            | Ty::Array { .. }
            | Ty::Vec { .. }
            | Ty::Map { .. } => false,
            Ty::Integer(_, _)
            | Ty::ByteArray { .. }
            | Ty::ByteVec { .. }
            | Ty::String
            | Ty::Float32
            | Ty::Float64
            | Ty::Boolean
            | Ty::Skip { .. } => true,
        }
    }
}

/// An enum variant can contain...
/// - A (possibly anonymous) struct
/// - Another Enum
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EnumVariant<L: LinkingScheme> {
    pub name: String,
    pub serde_name: String,
    pub template: Option<String>,
    pub value: Option<L::TypeLink>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Enum<L: LinkingScheme> {
    pub type_name: String,
    pub serde_type_name: String,
    pub variants: Vec<EnumVariant<L>>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Struct<L: LinkingScheme> {
    pub type_name: String,
    pub serde_type_name: String,
    pub template: Option<String>,
    pub fields: Vec<NamedField<L>>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Tuple<L: LinkingScheme> {
    pub template: Option<String>,
    pub fields: Vec<UnnamedField<L>>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NamedField<L: LinkingScheme> {
    pub display_name: String,
    pub serde_display_name: String,
    pub silent: bool,
    pub value: L::TypeLink,
    pub doc: String,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct UnnamedField<L: LinkingScheme> {
    pub value: L::TypeLink,
    pub silent: bool,
    pub doc: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[allow(non_camel_case_types)]
pub enum IntegerType {
    i8,
    i16,
    i32,
    i64,
    i128,
    u8,
    u16,
    u32,
    u64,
    u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum IntegerDisplay {
    Hex,
    #[default]
    Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, BorshDeserialize, BorshSerialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ByteDisplay {
    #[default]
    Hex,
    Decimal,
    Bech32 {
        #[cfg_attr(feature = "serde", serde(with = "hrp_serde"))]
        #[borsh(
            serialize_with = "hrp_borsh::borsh_serialize",
            deserialize_with = "hrp_borsh::borsh_deserialize"
        )]
        prefix: Hrp,
    },
    Bech32m {
        #[cfg_attr(feature = "serde", serde(with = "hrp_serde"))]
        #[borsh(
            serialize_with = "hrp_borsh::borsh_serialize",
            deserialize_with = "hrp_borsh::borsh_deserialize"
        )]
        prefix: Hrp,
    },
    Base58,
}

mod hrp_borsh {
    use bech32::Hrp;
    use borsh::{BorshDeserialize, BorshSerialize};

    pub fn borsh_serialize<W: borsh::io::Write>(
        hrp: &Hrp,
        w: &mut W,
    ) -> Result<(), borsh::io::Error> {
        let s = hrp.as_str();
        BorshSerialize::serialize(&s, w)
    }

    pub fn borsh_deserialize<R: borsh::io::Read>(r: &mut R) -> Result<Hrp, borsh::io::Error> {
        let s: String = BorshDeserialize::deserialize_reader(r)?;
        Hrp::parse(&s).map_err(borsh::io::Error::other)
    }
}

#[cfg(feature = "serde")]
mod hrp_serde {
    use bech32::Hrp;
    use serde::de::{self, Unexpected};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(hrp: &Hrp, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = hrp.as_str();
        ser.serialize_str(s)
    }

    pub fn deserialize<'de, D>(d: D) -> Result<Hrp, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = <&str>::deserialize(d)?;
        Hrp::parse(s).map_err(|_| de::Error::invalid_value(Unexpected::Str(s), &"a valid HRP"))
    }
}

impl ByteDisplay {
    pub fn format(&self, input: &[u8], f: &mut impl core::fmt::Write) -> Result<(), FormatError> {
        match self {
            ByteDisplay::Hex => {
                f.write_str("0x")?;
                for byte in input {
                    write!(f, "{:02x}", byte)?;
                }
            }
            ByteDisplay::Decimal => {
                write!(f, "{:?}", input)?;
            }
            ByteDisplay::Bech32 { prefix } => {
                bech32::encode_to_fmt::<Bech32, _>(f, *prefix, input)?
            }
            ByteDisplay::Bech32m { prefix } => {
                bech32::encode_to_fmt::<Bech32m, _>(f, *prefix, input)?
            }
            ByteDisplay::Base58 => {
                let out = bs58::encode(input).into_string();
                f.write_str(&out)?;
            }
        }
        Ok(())
    }
}

pub trait ByteDisplayable {
    fn with_display(display: ByteDisplay) -> Link {
        Link::Immediate(Primitive::ByteVec { display })
    }
}

impl<T: OverrideSchema<Output = Vec<u8>>> ByteDisplayable for T {
    fn with_display(display: ByteDisplay) -> Link {
        Link::Immediate(Primitive::ByteVec { display })
    }
}
impl ByteDisplayable for Vec<u8> {
    fn with_display(display: ByteDisplay) -> Link {
        Link::Immediate(Primitive::ByteVec { display })
    }
}

impl<const N: usize> ByteDisplayable for [u8; N] {
    fn with_display(display: ByteDisplay) -> Link {
        Link::Immediate(Primitive::ByteArray { len: N, display })
    }
}
