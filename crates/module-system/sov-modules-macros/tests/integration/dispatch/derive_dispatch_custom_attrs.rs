mod modules;
use modules::{first_test_module, second_test_module};
use sov_modules_api::sov_wallet_format::compiled_schema::CompiledSchema;
use sov_modules_api::{DispatchCall, Event, Genesis, MessageCodec, Spec};
use sov_test_utils::TestSpec;

#[derive(Default, Genesis, DispatchCall, Event, MessageCodec)]
#[dispatch_call(derive(sov_modules_api::macros::UniversalWallet))]
struct Runtime<S: Spec> {
    pub first: first_test_module::FirstTestStruct<S>,
    pub second: second_test_module::SecondTestStruct<S>,
}

fn main() {
    let _ = CompiledSchema::of::<RuntimeCall<TestSpec>>();
}
