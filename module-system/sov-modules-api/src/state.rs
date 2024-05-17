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

impl<S: Spec, T> TxState<S> for T where
    T: StateReaderAndWriter<User> + StateWriter<Accessory> + EventContainer + GasMeter<S::Gas>
{
}
/// The state accessor used during genesis. It provides unrestricted
/// access to [`User`] and `Kernel` state, as well as limited visibility into [`Accessory`] state.  
pub trait GenesisState<S: Spec>: TxState<S> {}

impl<S: Spec, T> GenesisState<S> for T where
    T: TxState<S>
        + StateReaderAndWriter<User>
        // + StateReaderAndWriter<sov_state::Kernel>
        + StateWriter<Accessory>
        + EventContainer
        + GasMeter<S::Gas>
{
}
