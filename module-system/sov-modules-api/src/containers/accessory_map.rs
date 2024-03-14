use sov_modules_core::namespaces::Accessory;
use sov_state::codec::BorshCodec;

use super::map::NamespacedStateMap;

/// A container that maps keys to values stored as "accessory" state, outside of
/// the JMT.
///
/// # Type parameters
/// [`AccessoryStateMap`] is generic over:
/// - a key type `K`;
/// - a value type `V`;
/// - a  [`Codec`](`sov_modules_core::StateValueCodec`).
pub type AccessoryStateMap<K, V, Codec = BorshCodec> = NamespacedStateMap<Accessory, K, V, Codec>;
