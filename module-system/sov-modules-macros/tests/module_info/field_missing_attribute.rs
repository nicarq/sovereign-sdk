use sov_modules_api::{Context, Module, ModuleInfo, Spec, StateMap, WorkingSet};

#[derive(ModuleInfo)]
struct TestStruct<S: Spec> {
    #[address]
    pub address: S::Address,

    test_state1: StateMap<u32, u32>,

    #[state]
    test_state2: StateMap<Vec<u8>, u64>,
}

impl<S: Spec> Module for TestStruct<S> {
    type Spec = S;

    type Config = ();

    type CallMessage = ();

    type Event = ();

    fn call(
        &self,
        _message: Self::CallMessage,
        _context: &Context<Self::Spec>,
        _working_set: &mut WorkingSet<Self::Spec>,
    ) -> Result<sov_modules_api::CallResponse, sov_modules_api::Error> {
        todo!()
    }
}

fn main() {}
