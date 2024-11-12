//! Runtime call message definitions.

use strum::{VariantArray, VariantNames};

use super::ModuleInfo;
use crate::common::ModuleError;
use crate::module::{CallResponse, Context, Spec};
use crate::{GasMeter, MeteredBorshDeserializeError, ModuleId, StateProvider, WorkingSet};

/// A helper trait for working with enums like our generated `RuntimeCall` whose variants are tuples
/// containing a single field
pub trait NestedEnumUtils: VariantNames + AsRef<str> {
    /// An enum that consists of just the discriminant of the call message with no data.
    type Discriminants: VariantNames
        + VariantArray
        + Into<&'static str>
        + AsRef<str>
        + Clone
        + Copy
        + std::fmt::Debug;

    /// Returns the discriminant of the call message.
    fn discriminant(&self) -> Self::Discriminants;

    /// Returns the inner enum associated with this variant as a [`std::any::Any`]
    fn raw_contents(&self) -> &dyn std::any::Any;

    /// Returns the inner enum associated with this variant as a type-safe [`InnerEnumVariant`]
    fn contents(&self) -> InnerEnumVariant<'_> {
        InnerEnumVariant(self.raw_contents())
    }
}

/// The inner contents of nested enum.
pub struct InnerEnumVariant<'a>(&'a dyn std::any::Any);

impl<'a> InnerEnumVariant<'a> {
    /// Returns the contents of the nested enum.
    pub fn inner(&self) -> &dyn std::any::Any {
        self.0
    }

    /// A type-unsafe constructor for use in testing
    #[cfg(feature = "test-utils")]
    pub fn new_for_test(contents: &'a dyn std::any::Any) -> Self {
        Self(contents)
    }
}

/// A trait that needs to be implemented for any call message.
pub trait DispatchCall: Send + Sync {
    /// The context of the call
    type Spec: Spec;

    /// The concrete type that will decode into the call message of the module.
    type Decodable: Send + Sync + NestedEnumUtils;

    /// Encode a [`Self::Decodable`]
    fn encode(decodable: &Self::Decodable) -> Vec<u8>;

    /// Decodes serialized call message
    fn decode_call(
        serialized_message: &[u8],
        meter: &mut impl GasMeter<<Self::Spec as Spec>::Gas>,
    ) -> Result<Self::Decodable, MeteredBorshDeserializeError<<Self::Spec as Spec>::Gas>>;

    /// Dispatches a call message to the appropriate module.
    fn dispatch_call<I: StateProvider<Self::Spec>>(
        &self,
        message: Self::Decodable,
        state: &mut WorkingSet<Self::Spec, I>,
        context: &Context<Self::Spec>,
    ) -> Result<CallResponse, ModuleError>;

    /// Returns the ID of the dispatched module.
    fn module_id(&self, message: &Self::Decodable) -> &ModuleId;

    /// Returns the ID of the dispatched module.
    fn module_info(
        &self,
        discriminant: <Self::Decodable as NestedEnumUtils>::Discriminants,
    ) -> &dyn ModuleInfo<Spec = Self::Spec>;
}
