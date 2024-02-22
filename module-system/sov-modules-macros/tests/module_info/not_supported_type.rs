use sov_modules_api::{ModuleInfo, Spec, StateMap};

#[derive(ModuleInfo)]
struct TestStruct<S: Spec> {
    #[address]
    address: S::Address,

    #[state]
    test_state1: [usize; 22],

    #[state]
    test_state2: StateMap<u32, u32>,
}

fn main() {}
