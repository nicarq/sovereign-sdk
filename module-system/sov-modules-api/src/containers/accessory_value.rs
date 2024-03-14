use sov_modules_core::namespaces::Accessory;
use sov_state::codec::BorshCodec;

use super::value::NamespacedStateValue;

/// Container for a single value stored as "accessory" state, outside of the
/// JMT.
pub type AccessoryStateValue<V, Codec = BorshCodec> = NamespacedStateValue<Accessory, V, Codec>;
