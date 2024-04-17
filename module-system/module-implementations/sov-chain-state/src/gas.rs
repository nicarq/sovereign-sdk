use sov_modules_api::macros::config_constant;
use sov_modules_api::{DaSpec, Gas, GasArray, KernelWorkingSet, Spec};

use crate::ChainState;

/// Defines constants used by the chain state module for gas price computation
impl<S: Spec, Da: DaSpec> ChainState<S, Da> {
    /// Specifies the initial base fee per gas for the genesis block.
    ///
    /// # TODO
    /// This method should be converted in a constant time constructor. The current implementation of the
    /// [`config_constant`] macro cannot be used to define [`sov_modules_api::GasPrice`] constants, so this will probably
    /// require a new proc-macro, see `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/475>`.
    /// Besides, this value is currently defined at genesis and is stored in the module state. This
    /// should be changed in the future to be a constant value defined in the `constants{.test}.json` file
    /// see `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/469>`
    ///
    ///
    /// # Note
    /// This constant is the same as the `INITIAL_BASE_FEE_PER_GAS` constant
    /// defined in the EIP-1559 specification its default value is `[1, 1]`.
    ///
    /// # Safety
    /// This method panics if the initial gas price is not set at genesis
    pub fn initial_base_fee_per_gas(
        &self,
        kernel_working_set: &mut KernelWorkingSet<S>,
    ) -> <S::Gas as Gas>::Price {
        self.initial_base_fee_per_gas
            .get(kernel_working_set)
            .expect("The initial gas price should be set at genesis")
    }

    /// Specifies the initial gas limit for the genesis block.
    /// This value is retrieved from the config file and is then converted to a [`sov_modules_api::GasUnit`] at runtime
    ///
    /// # TODO
    /// This method should be converted in a constant time constructor. The current implementation of the
    /// [`config_constant`] macro cannot be used to define [`sov_modules_api::GasUnit`] constants, so this will probably
    /// require a new proc-macro `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/475>`.
    ///
    /// # Note
    /// This constant is the same as the `INITIAL_BASE_FEE_PER_GAS` constant
    /// defined in the EIP-1559 specification its default value is `[1, 1]`.
    pub fn initial_gas_limit() -> S::Gas {
        #[config_constant]
        const INITIAL_GAS_LIMIT: &[u64];

        S::Gas::from_slice(INITIAL_GAS_LIMIT)
    }
}
