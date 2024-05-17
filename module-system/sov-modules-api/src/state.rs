//! Defines traits for storage access

use sov_state::{Accessory, EventContainer, StateReaderAndWriter, StateWriter, User};

use crate::{GasMeter, Spec};

/// The state accessor used during transaction execution. It provides unrestricted
/// access to [`User`]-space state, as well as limited visibility into the `Kernel` state.
pub trait TxState<S: Spec>:
    StateReaderAndWriter<User>
    // + StateReader<Kernel> TODO: <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/596>
    + StateWriter<Accessory>
    + EventContainer
    + GasMeter<S::Gas>
{
}
