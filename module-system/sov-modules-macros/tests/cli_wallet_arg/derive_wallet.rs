use clap::Parser;
use sov_modules_api::cli::JsonStringArg;
use sov_modules_api::macros::{CliWallet, CliWalletArg, DefaultRuntime};
use sov_modules_api::{
    CallResponse, Context, DispatchCall, Error, Genesis, MessageCodec, Module, ModuleInfo, Spec,
    StateValue, WorkingSet,
};
type DefaultSpec = sov_modules_api::default_spec::DefaultSpec<sov_mock_zkvm::MockZkVerifier>;

pub mod first_test_module {
    use super::*;

    #[derive(
        CliWalletArg,
        Debug,
        PartialEq,
        borsh::BorshDeserialize,
        borsh::BorshSerialize,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub struct MyStruct {
        pub first_field: u32,
        pub str_field: String,
    }

    #[derive(ModuleInfo)]
    pub struct FirstTestStruct<S: Spec> {
        #[address]
        pub address: S::Address,

        #[state]
        pub state_in_first_struct: StateValue<u8>,
    }

    impl<S: Spec> Module for FirstTestStruct<S> {
        type Spec = S;
        type Config = ();
        type CallMessage = MyStruct;
        type Event = ();

        fn genesis(
            &self,
            _config: &Self::Config,
            _working_set: &mut WorkingSet<S>,
        ) -> Result<(), Error> {
            Ok(())
        }

        fn call(
            &self,
            _msg: Self::CallMessage,
            _context: &Context<Self::Spec>,
            _working_set: &mut WorkingSet<S>,
        ) -> Result<CallResponse, Error> {
            Ok(CallResponse::default())
        }
    }
}

pub mod second_test_module {
    use super::*;

    #[derive(ModuleInfo)]
    pub struct SecondTestStruct<S: Spec> {
        #[address]
        pub address: S::Address,

        #[state]
        pub state_in_second_struct: StateValue<u8>,
    }

    #[derive(
        CliWalletArg,
        Debug,
        PartialEq,
        borsh::BorshDeserialize,
        borsh::BorshSerialize,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub enum MyEnum {
        Foo { first_field: u32, str_field: String },
        Bar(u8),
    }

    impl<S: Spec> Module for SecondTestStruct<S> {
        type Spec = S;
        type Config = ();
        type CallMessage = MyEnum;
        type Event = ();

        fn genesis(
            &self,
            _config: &Self::Config,
            _working_set: &mut WorkingSet<S>,
        ) -> Result<(), Error> {
            Ok(())
        }

        fn call(
            &self,
            _msg: Self::CallMessage,
            _context: &Context<Self::Spec>,
            _working_set: &mut WorkingSet<S>,
        ) -> Result<CallResponse, Error> {
            Ok(CallResponse::default())
        }
    }
}

#[derive(Genesis, DispatchCall, MessageCodec, DefaultRuntime, CliWallet)]
#[serialization(borsh::BorshDeserialize, borsh::BorshSerialize)]
pub struct Runtime<S: Spec> {
    pub first: first_test_module::FirstTestStruct<S>,
    pub second: second_test_module::SecondTestStruct<S>,
}

fn main() {
    let expected_foo = RuntimeCall::first(first_test_module::MyStruct {
        first_field: 1,
        str_field: "hello".to_string(),
    });
    let foo_from_cli: RuntimeSubcommand<JsonStringArg, DefaultSpec> =
        <RuntimeSubcommand<JsonStringArg, DefaultSpec>>::try_parse_from(&[
            "main",
            "first",
            "--json",
            r#"{"first_field": 1, "str_field": "hello"}"#,
            "--chain-id",
            "0",
        ])
        .expect("parsing must succed")
        .into();
    let foo_ir: RuntimeMessage<JsonStringArg, DefaultSpec> = foo_from_cli.try_into().unwrap();
    assert_eq!(expected_foo, foo_ir.try_into().unwrap());

    let expected_bar = RuntimeCall::second(second_test_module::MyEnum::Bar(2));
    let bar_from_cli: RuntimeSubcommand<JsonStringArg, DefaultSpec> =
        <RuntimeSubcommand<JsonStringArg, DefaultSpec>>::try_parse_from(&[
            "main",
            "second",
            "--json",
            r#"{"Bar": 2}"#,
            "--chain-id",
            "0",
        ])
        .expect("parsing must succed")
        .into();
    let bar_ir: RuntimeMessage<JsonStringArg, DefaultSpec> = bar_from_cli.try_into().unwrap();

    assert_eq!(expected_bar, bar_ir.try_into().unwrap());
}
