use std::any::TypeId;
use std::collections::HashMap;
mod container;
mod primitive;
pub use container::Container;
pub use primitive::Primitive;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
mod schema_impls;
#[cfg(test)]
mod tests;

use crate::display::{Context as DisplayContext, DisplayVisitor, FormatError};
#[cfg(feature = "serde")]
use crate::json_to_borsh::{Context as EncodeContext, EncodeError, EncodeVisitor};
use crate::ty::{LinkingScheme, Ty};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct IndexLinking;

impl LinkingScheme for IndexLinking {
    type TypeLink = Link;
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Link {
    ByIndex(usize),
    Immediate(Primitive),
    Placeholder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MaybePartialLink {
    Partial(Link),
    Complete(Link),
}

impl MaybePartialLink {
    fn into_inner(self) -> Link {
        match self {
            MaybePartialLink::Partial(link) => link,
            MaybePartialLink::Complete(link) => link,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemId(pub TypeId);

impl ItemId {
    pub fn of<T: 'static + SchemaGenerator>() -> Self {
        T::id_override().unwrap_or(ItemId(TypeId::of::<T>()))
    }
}

#[derive(Default, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Schema {
    types: Vec<Ty<IndexLinking>>,
    /// A map from the type ID of an item to its index in the types array. Note that primitives and "virtual" structs/tuples
    /// (i.e. the contents of an enum variant) are not included in this map.
    /// Only used during schema construction.
    #[cfg_attr(feature = "serde", serde(skip))]
    known_types: HashMap<ItemId, usize>,

    /// Keeps track of all the types which are partially constructed. By the end of schema generation, this
    /// must be empty.
    #[cfg_attr(feature = "serde", serde(skip))]
    under_construction: HashMap<ItemId, usize>,
}

/// A schema, representing a type (i.e. rust code) as a data structure.
/// The schema allows the type's borsh serialization to be displayed as a human readable string,
/// and the type's JSON serialisation to be re-serialised to borsh without depending on the
/// original Rust type.
/// It is also serialisable and therefore can be imported and used with non-Rust languages, enabling
/// foreign language toolkits to implement the same functionality as above.
///
/// A schema can be instantiated for any type that implements either `SchemaGenerator` or
/// `OverrideSchema`. In turn, `SchemaGenerator` is intended to be automatically derived using the
/// `UniversalWallet` macro.
impl Schema {
    pub fn of<T: SchemaGenerator>() -> Self {
        let mut schema = Self::default();
        T::write_schema(&mut schema);
        assert!(
            schema.under_construction.is_empty(),
            "Schema generation left some types partially constructed. This is a bug in the schema. {:?}",
            schema
        );
        schema
    }

    #[cfg(feature = "serde")]
    pub fn from_json(input: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(input)
    }

    /// Use the schema to display the provided input
    pub fn display(&self, input: &[u8]) -> Result<String, FormatError> {
        let mut output = String::new();
        let input = &mut &input[..];
        let mut visitor = DisplayVisitor::new(input, &mut output);
        // Use `?` to return the error if it exists, but drop the inner Option. If it's `None` and the input is non-empty, we'll return
        // an error thanks to the check that `visitor.has_displayed_whole_input()`.
        let _ = self
            .types
            .first()
            .map(|ty: &Ty<IndexLinking>| ty.visit(self, &mut visitor, DisplayContext::default()))
            .transpose()?;

        if !visitor.has_displayed_whole_input() {
            return Err(FormatError::UnusedInput);
        }
        Ok(output)
    }

    /// Use the schema to convert the provided serde-compatible JSON string into borsch
    #[cfg(feature = "serde")]
    pub fn json_to_borsh(&self, input: &str) -> Result<Vec<u8>, EncodeError> {
        let mut output = Vec::new();

        let mut visitor = EncodeVisitor::new(&mut output)?;

        let _ = self
            .types
            .first()
            .map(|ty: &Ty<IndexLinking>| {
                println!("Visiting type {:?}!", ty);
                ty.visit(self, &mut visitor, EncodeContext::new(input)?)
            })
            .transpose()?;

        Ok(output)
    }

    fn add_type_if_absent(&mut self, ty: Ty<IndexLinking>, item_id: ItemId) -> Link {
        if let Some(location) = self.known_types.get(&item_id) {
            return Link::ByIndex(*location);
        }
        let location = self.types.len();
        self.known_types.insert(item_id, location);
        self.types.push(ty);
        Link::ByIndex(location)
    }

    pub fn types(&self) -> &[Ty<IndexLinking>] {
        &self.types
    }

    fn find_item_by_id(&self, item_id: &ItemId) -> Option<usize> {
        self.known_types.get(item_id).copied()
    }

    /// Link a child type to its parent, panicking if the parent type is not in the schema or if the parent type has no more placeholders.
    fn link_child_to_parent(&mut self, parent: ItemId, child: Link) {
        let idx = self.known_types.get(&parent).unwrap_or_else(|| panic!("Tried to link a child to a parent ({:?}) that the schema doesn't have. This is a bug in a hand-written schema.", parent));

        let remaining_children = *self.under_construction.get(&parent).unwrap_or_else(|| panic!("Tried to link too many children to parent ({:?}). This is a bug in a hand-written schema.", parent));
        if remaining_children == 1 {
            self.under_construction.remove(&parent);
        } else {
            self.under_construction
                .insert(parent, remaining_children - 1);
        }
        self.types[*idx].fill_next_placholder(child);
    }

    /// Get a link to the given type, adding it to the top-level schema if necessary.
    /// Unlike all other methods in this crate, the linked type returned by this method is allowed to be only partially generated.
    ///
    /// It is the responsibility of the caller to complete the returned link.
    fn get_partial_link_to(
        &mut self,
        item: Item<IndexLinking>,
        item_id: ItemId,
    ) -> MaybePartialLink {
        match item {
            Item::Container(c) => {
                if let Some(location) = self.find_item_by_id(&item_id) {
                    MaybePartialLink::Complete(Link::ByIndex(location))
                } else {
                    let location = self.types.len();
                    self.known_types.insert(item_id.clone(), location);
                    let num_children = c.num_children();
                    self.types.push(c.into());
                    if num_children != 0 {
                        self.under_construction.insert(item_id, num_children);
                        MaybePartialLink::Partial(Link::ByIndex(location))
                    } else {
                        MaybePartialLink::Complete(Link::ByIndex(location))
                    }
                }
            }
            Item::Atom(primitive) => MaybePartialLink::Complete(Link::Immediate(primitive)),
        }
    }
}

pub enum Item<L: LinkingScheme> {
    Container(Container<L>),
    Atom(Primitive),
}

/// Generate the schema for a type.
/// For complex types, this should typically be derived with a macro,
/// rather than implemented by hand.
/// This is also automatically implemented for all types implementing `OverrideSchema`.
pub trait SchemaGenerator: Sized + 'static {
    /// Ensure that each type contained in the outer type (i.e. the type of each struct/tuple field) is added to the schema,
    /// and return a `Link` connecting the child to the parent.
    ///
    /// Ideally, this function would return something like `Box<dyn SchemaGenerator>`.
    /// Unfortunately, we need to return *types*, not instances (because we don't want to
    /// add a `Default` bound on all types that implement SchemaGenerator) which Rustc doesn't like.
    /// So, we have a slightly messier signature where the type is expected to register each of its child
    /// types with the schema directly rather than returning them to the caller for future registration.
    fn get_child_links(_schema: &mut Schema) -> Vec<Link>;

    /// Generate the "scaffolding" of the item. If the item is a primtive, this is just the corresponding primtive.
    /// If the type is composed of other types, this is the container with all links set to [`Link::Placeholder`].
    fn scaffold() -> Item<IndexLinking>;

    /// Writes the type to the top-level schema if it is not already present and returns a link to it.
    ///
    /// Any child types will have their schemas generated as well, but the placement of those types is left
    /// to the discretion of the implementation - they may or may not appear at the top level of the schema.
    fn write_schema(schema: &mut Schema) -> Link {
        let item = Self::scaffold();
        let item_id = ItemId::of::<Self>();
        match item {
            Item::Atom(primitive) => schema.add_type_if_absent(primitive.into(), item_id),
            Item::Container(container) => {
                let link = schema.get_partial_link_to(Item::Container(container), item_id.clone());
                if let MaybePartialLink::Complete(link) = link {
                    return link;
                }

                for child in Self::get_child_links(schema) {
                    schema.link_child_to_parent(item_id.clone(), child);
                }
                link.into_inner()
            }
        }
    }

    /// Gets a link to the type, writing the type to the schema if necessary.
    fn make_linkable(schema: &mut Schema) -> Link {
        match Self::scaffold() {
            Item::Container(_) => Self::write_schema(schema),
            Item::Atom(atom) => Link::Immediate(atom),
        }
    }

    /// Override the type ID of the item. This should typically not be written by hand. Instead,
    /// use the [`OverrideSchema`] trait.
    fn id_override() -> Option<ItemId> {
        None
    }
}

/// Establish that this type should use the `Output` type to generate its schema.
/// This is appropriate for cases where different types represent the same kind of data structure.
/// For instance, HashMap and BTreeMap both represent a `Container::Map` in the data model of the
/// schema; their internal implementation differences don't affect the shape of their schemas.
///
/// Note that, for types to be considered equivalent in the schema, their borsh and JSON
/// serialisations must both also be equivalent.
pub trait OverrideSchema {
    type Output: SchemaGenerator;
}

impl<T: OverrideSchema + 'static> SchemaGenerator for T {
    fn scaffold() -> Item<IndexLinking> {
        <Self as OverrideSchema>::Output::scaffold()
    }
    fn get_child_links(schema: &mut Schema) -> Vec<Link> {
        <Self as OverrideSchema>::Output::get_child_links(schema)
    }
    fn id_override() -> Option<ItemId> {
        <Self as OverrideSchema>::Output::id_override()
    }
    fn make_linkable(schema: &mut Schema) -> Link {
        <Self as OverrideSchema>::Output::make_linkable(schema)
    }
    fn write_schema(schema: &mut Schema) -> Link {
        <Self as OverrideSchema>::Output::write_schema(schema)
    }
}
