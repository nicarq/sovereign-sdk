use sov_modules_core::namespaces::Kernel;
use sov_state::codec::BorshCodec;

use super::value::NamespacedStateValue;

/// Container for a single value which is only accesible in the kernel.
pub type KernelStateValue<V, Codec = BorshCodec> = NamespacedStateValue<Kernel, V, Codec>;
