use crate::impl_hash32_type;

impl_hash32_type!(ModuleId, ModuleIdBech32, "module_");

// A hack to ensure that paths relative to sov_modules_api` needed by the macro
// exist.
#[doc(hidden)]
mod sov_modules_api {
    pub use sov_universal_wallet;

    pub use crate::macros;
}
