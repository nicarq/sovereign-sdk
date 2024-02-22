mod modules;
use modules::{first_test_module, second_test_module};
use sov_modules_api::macros::DefaultRuntime;
use sov_modules_api::{DispatchCall, Spec, Event, Genesis, MessageCodec};
type DefaultSpec = sov_modules_api::default_spec::DefaultSpec<sov_mock_zkvm::MockZkVerifier>;

#[derive(Genesis, DispatchCall, Event, MessageCodec, DefaultRuntime)]
#[serialization(borsh::BorshDeserialize, borsh::BorshSerialize)]
struct Runtime<S: Spec> {
    pub first: first_test_module::FirstTestStruct<S>,
    pub second: second_test_module::SecondTestStruct<S>,
}

fn main() {
    // Check to see if the runtime events are getting initialized correctly
    let _event = RuntimeEvent::<DefaultSpec>::first(first_test_module::Event::FirstModuleEnum1(10));
    let _event = RuntimeEvent::<DefaultSpec>::first(first_test_module::Event::FirstModuleEnum2);
    let _event =
        RuntimeEvent::<DefaultSpec>::first(first_test_module::Event::FirstModuleEnum3(vec![1; 3]));
    let _event = RuntimeEvent::<DefaultSpec>::second(second_test_module::Event::SecondModuleEnum);
}
