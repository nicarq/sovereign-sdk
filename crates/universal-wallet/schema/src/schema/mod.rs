use std::any::TypeId;
use std::collections::HashMap;
use std::fmt::Debug;
mod container;
mod primitive;
pub mod safe_string;
use borsh::{BorshDeserialize, BorshSerialize};
pub use container::Container;
use nmt_rs::simple_merkle::db::MemDb;
use nmt_rs::simple_merkle::tree::{MerkleHash, MerkleTree};
use nmt_rs::TmSha2Hasher;
pub use primitive::Primitive;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
mod schema_impls;
#[cfg(test)]
mod tests;

use thiserror::Error;

use crate::display::{Context as DisplayContext, DisplayVisitor, FormatError};
#[cfg(feature = "serde")]
use crate::json_to_borsh::{Context as EncodeContext, EncodeError, EncodeVisitor};
use crate::ty::{LinkingScheme, Ty};

#[derive(Debug, Error)]
pub enum SchemaError {
    #[error(transparent)]
    FormatError(#[from] FormatError),
    #[error(transparent)]
    BorshError(#[from] borsh::io::Error),
    #[cfg(feature = "serde")]
    #[error(transparent)]
    EncodeError(#[from] EncodeError),
    #[error("Rollup type {0:?} was missing from schema")]
    MissingRollupRoot(RollupRoots),
    #[error("Index {0} not found in schema")]
    InvalidIndex(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct IndexLinking;

impl LinkingScheme for IndexLinking {
    type TypeLink = Link;
}

// TODO: Some type safety for fully-constructed schemas.
// It should be possible to use the type system to ensure at compile-time that
// a) constructed Schemas do not have any Link::Placeholder; and
// b) it is not possible to call construction methods (the ones that edit the links) on a finished
// Schema.
// This could be done with, for example, an intermediate SchemaUnderConstruction type using a
// ConstrutionIndexLinking, which implements into::<Schema>().
//
// Right now this is mostly achieved using member visibility (nobody outside can call the private
// construction methods) and sanity checking (on a derived schema, if under_construction is empty,
// there won't be any placeholders); but a separate type would provide a stronger guarantee.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
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

/// This newtype is mainly necessary to allow the schema to derive Debug ergonomically
#[derive(Default)]
pub struct MerkleTreeCache(Option<MerkleTree<MemDb<[u8; 32]>, TmSha2Hasher>>);

impl Debug for MerkleTreeCache {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemId(pub TypeId);

impl ItemId {
    pub fn of<T: 'static + SchemaGenerator>() -> Self {
        T::id_override().unwrap_or(ItemId(TypeId::of::<T>()))
    }
}

/// Not enforced in the types, but the expected convention that should be followed when generating
/// the schema.
#[derive(Debug, Copy, Clone)]
pub enum RollupRoots {
    Transaction = 0,
    UnsignedTransaction = 1,
    RuntimeCall = 2,
}

/// A schema, representing set of types (i.e. rust code) as a data structure.
/// The schema allows any included type's borsh serialization to be displayed as a human readable string,
/// and the type's JSON serialisation to be re-serialised to borsh without depending on the
/// original Rust type.
/// It is also serialisable and therefore, once generated for a rollup, can be imported and used with
/// non-Rust languages, enabling toolkits in any language to implement the same functionality as above.
///
/// A schema can be instantiated for any type that implements either `SchemaGenerator` or
/// `OverrideSchema`. In turn, `SchemaGenerator` is intended to be automatically derived using the
/// `UniversalWallet` macro.
#[derive(Default, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Schema {
    /// The types described by this schema. This is an array of type descriptions, where complex
    /// types refer to their sub-types by index within the array.
    /// Any of the types here can be used for schema operations (such as borsh-to-human or
    /// json-to-borsh reserialisations).
    types: Vec<Ty<IndexLinking>>,

    /// A mapping from the complex root types that parametrized the schema generation invocation (in
    /// order, skipping primitives) to the actual indices they ended up at in the type array above.
    root_type_indices: Vec<usize>,

    /// The chain metadata hash. Hashed with the root of the schema's merkle tree to
    /// calculate the final chain ID.
    /// The metadata can be any arbitrary type, as long as it can be serialized into a bytevec for
    /// hashing.
    metadata_hash: [u8; 32],

    /// Cached chain ID value, calculated by hashing the schema's merkle root with the chain nonce
    #[cfg_attr(feature = "serde", serde(skip))]
    chain_hash: Option<[u8; 32]>,

    /// Cached (lazily-constructed) merkelization of the entire schema.
    #[cfg_attr(feature = "serde", serde(skip))]
    merkle_tree: MerkleTreeCache,

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

impl Schema {
    /// Instantiate a schema for a single type.
    /// This root type will be at index 0
    pub fn of_single_type<T: SchemaGenerator>() -> Self {
        // TODO: this could easily be implemented with a macro for N types for any N >= 1, if ever needed
        let mut schema = Self::default();
        let link = T::write_schema(&mut schema);
        schema.push_root_link(link);
        assert!(
            schema.under_construction.is_empty(),
            "Schema generation left some types partially constructed. This is a bug in the schema. {:?}",
            schema
        );
        schema
    }

    /// Instantiate a schema for a standard set of rollup types: its complete transaction, its
    /// unsigned transaction, and its call message type.
    /// The types will be accessible using the indices stored in root_type_indices (in the above
    /// order); they can also be queried using the `RollupRoots` enum through the `_rollup`-tagged
    /// functions on the schema
    pub fn of_rollup_types_with_metadata<
        Metadata: BorshSerialize,
        Transaction: SchemaGenerator,
        UnsignedTransaction: SchemaGenerator,
        RuntimeCall: SchemaGenerator,
    >(
        chain_metadata: &Metadata,
    ) -> Result<Self, SchemaError> {
        let hasher = TmSha2Hasher::new();
        let metadata_hash = hasher.hash_leaf(&borsh::to_vec(&chain_metadata)?);
        let mut schema = Self {
            metadata_hash,
            ..Default::default()
        };
        let link = Transaction::write_schema(&mut schema);
        schema.push_root_link(link);
        let link = UnsignedTransaction::write_schema(&mut schema);
        schema.push_root_link(link);
        let link = RuntimeCall::write_schema(&mut schema);
        schema.push_root_link(link);
        assert!(
            schema.under_construction.is_empty(),
            "Schema generation left some types partially constructed. This is a bug in the schema. {:?}",
            schema
        );
        Ok(schema)
    }

    /// Returns the chain ID calculated using the merkle root of all the schema types, combined
    /// with a chain-specific nonce value.
    /// This allows the chain ID to be used for verification of the schema (and thus verification
    /// that a transaction claiming to correspond to a given schema will have the effect it claims).
    pub fn chain_hash(&mut self) -> Result<[u8; 32], SchemaError> {
        match self.chain_hash {
            Some(id) => Ok(id),
            None => {
                let merkle_root = match &mut self.merkle_tree.0 {
                    Some(tree) => tree.root(),
                    None => {
                        let mut tree = MerkleTree::new();
                        for ty in &self.types {
                            tree.push_raw_leaf(&borsh::to_vec(ty)?)
                        }
                        let root = tree.root();
                        self.merkle_tree = MerkleTreeCache(Some(tree));
                        root
                    }
                };
                let hasher = TmSha2Hasher::new();
                Ok(hasher.hash_nodes(&merkle_root, &hasher.hash_leaf(&self.metadata_hash)))
            }
        }
    }

    pub fn metadata_hash(&self) -> &[u8; 32] {
        &self.metadata_hash
    }

    #[cfg(feature = "serde")]
    pub fn from_json(input: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(input)
    }

    pub fn rollup_expected_index(&self, rollup_type: RollupRoots) -> Result<usize, SchemaError> {
        self.root_type_indices
            .get(rollup_type as usize)
            .copied()
            .ok_or(SchemaError::MissingRollupRoot(rollup_type))
    }

    /// Use the schema to display the given type using the provided borsh encoded input
    pub fn display(&self, type_index: usize, input: &[u8]) -> Result<String, SchemaError> {
        let mut output = String::new();
        let input = &mut &input[..];
        let mut visitor = DisplayVisitor::new(input, &mut output);
        self.types
            .get(type_index)
            .ok_or(SchemaError::InvalidIndex(type_index))?
            .visit(self, &mut visitor, DisplayContext::default())?;

        if !visitor.has_displayed_whole_input() {
            return Err(FormatError::UnusedInput.into());
        }
        Ok(output)
    }

    /// Use the schema to convert a serde-compatible JSON string of the given type into its borsh
    /// encoding
    #[cfg(feature = "serde")]
    pub fn json_to_borsh(&self, type_index: usize, input: &str) -> Result<Vec<u8>, SchemaError> {
        let mut output = Vec::new();

        let mut visitor = EncodeVisitor::new(&mut output)?;

        self.types
            .get(type_index)
            .ok_or(SchemaError::InvalidIndex(type_index))?
            .visit(self, &mut visitor, EncodeContext::new(input)?)?;

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

    pub fn root_types(&self) -> &[usize] {
        &self.root_type_indices
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
                    let num_children = c.num_children();
                    let location = self.types.len();
                    self.known_types.insert(item_id.clone(), location);
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

    /// After generating a root type, register it with the schema for ease of reference. Sets
    /// the canonical "root links", so has to be carefully called in the right order (normally,
    /// immediately after root type construction, with the link to the newly created type).
    /// No-op for primitive links.
    /// Panics on placeholder links.
    fn push_root_link(&mut self, link: Link) {
        match link {
            Link::ByIndex(i) => self.root_type_indices.push(i),
            Link::Immediate(..) => {},
            Link::Placeholder => panic!("Attempted to register a placeholder link as a schema root - are you passing the right link?"),
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

    /// Writes the type to the schema if it is not already present and returns a link to it.
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
