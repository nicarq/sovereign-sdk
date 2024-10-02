mod modules;
use modules::{first_test_module, second_test_module};
use sov_modules_api::{DispatchCall, Event, Genesis, MessageCodec, Spec};

#[derive(Default, Genesis, DispatchCall, Event, MessageCodec)]
#[event(no_default_attrs)]
struct Runtime<S: Spec> {
    pub first: first_test_module::FirstTestStruct<S>,
    pub second: second_test_module::SecondTestStruct<S>,
}

// This test fails to compile because required traits are not implemented for the
// generated `RuntimeEvent` enum.
fn main() {}
