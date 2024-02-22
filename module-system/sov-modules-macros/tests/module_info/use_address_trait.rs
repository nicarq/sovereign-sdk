//! Regression for <https://github.com/Sovereign-Labs/sovereign-sdk/issues/635>.

#![allow(unused_imports)]

use sov_modules_api::{ModuleInfo, RollupAddress, Spec};

#[derive(ModuleInfo)]
struct TestModule<S: Spec> {
    #[address]
    admin: S::Address,
}

fn main() {}
