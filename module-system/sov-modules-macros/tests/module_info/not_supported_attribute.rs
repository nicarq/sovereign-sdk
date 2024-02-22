use sov_modules_api::{ModuleInfo, CryptoSpec, Spec, StateMap};

#[derive(ModuleInfo)]
struct TestStruct<S: Spec> {
    #[address]
    address: S::Address,

    // Unsupported attributes should be ignored to guarantee compatibility with
    // other macros.
    #[allow(dead_code)]
    #[state]
    test_state1: StateMap<u32, String>,

    #[state]
    test_state2: StateMap<<S::CryptoSpec as CryptoSpec>::PublicKey, String>,
}

fn main() {}
