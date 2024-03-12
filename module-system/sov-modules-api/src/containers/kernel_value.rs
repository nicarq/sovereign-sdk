use sov_modules_core::namespaces::Kernel;
use sov_state::codec::BorshCodec;

use super::value::GenericStateValue;

/// Container for a single value which is only accesible in the kernel.
pub type KernelStateValue<V, Codec = BorshCodec> = GenericStateValue<Kernel, V, Codec>;
