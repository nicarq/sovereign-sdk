use std::fmt::Debug;

use sov_modules_api::macros::CliWalletArg;
use sov_modules_api::prelude::clap::Parser;
use sov_modules_api::CliWalletArg;

fn assert_cli_wallet_arg<T>(expected: T, input: &[&str])
where
    T: CliWalletArg + Debug + PartialEq,
    T::CliStringRepr: Parser,
{
    let actual = <T as CliWalletArg>::CliStringRepr::try_parse_from(input)
        .unwrap_or_else(|error| {
            panic!(
                "Parsing {:?} should succeed (expected: {:?}), error: {:?}",
                input, expected, error
            )
        })
        .into();

    assert_eq!(expected, actual);
}

#[test]
fn enum_mixed_fields() {
    #[derive(CliWalletArg, Debug, PartialEq)]
    pub enum MyEnum {
        Foo { first_field: u32, str_field: String },
        Bar(u8),
    }

    assert_cli_wallet_arg(
        MyEnum::Foo {
            first_field: 1,
            str_field: "hello".to_string(),
        },
        &["myenum", "foo", "1", "hello"],
    );

    assert_cli_wallet_arg(MyEnum::Bar(2), &["myenum", "bar", "2"]);
}

#[test]
fn enum_named_fields() {
    #[derive(CliWalletArg, Debug, PartialEq)]
    pub enum MyEnum {
        Foo { first_field: u32, str_field: String },
        Bar { byte: u8 },
    }

    assert_cli_wallet_arg(
        MyEnum::Foo {
            first_field: 1,
            str_field: "hello".to_string(),
        },
        &["myenum", "foo", "1", "hello"],
    );

    assert_cli_wallet_arg(MyEnum::Bar { byte: 2 }, &["myenum", "bar", "2"]);
}

#[test]
fn enum_unnamed_fields() {
    #[derive(CliWalletArg, Debug, PartialEq)]
    pub enum MyEnum {
        Foo(u32, String),
        Bar(u8),
    }

    assert_cli_wallet_arg(
        MyEnum::Foo(1, "hello".to_string()),
        &["myenum", "foo", "1", "hello"],
    );

    assert_cli_wallet_arg(MyEnum::Bar(2), &["myenum", "bar", "2"]);
}

#[test]
fn struct_named_fields() {
    #[derive(CliWalletArg, Debug, PartialEq)]
    pub struct MyStruct {
        first_field: u32,
        str_field: String,
    }

    assert_cli_wallet_arg(
        MyStruct {
            first_field: 1,
            str_field: "hello".to_string(),
        },
        &["main", "my-struct", "1", "hello"],
    );
}

#[test]
fn struct_unnamed_fields() {
    #[derive(CliWalletArg, Debug, PartialEq)]
    pub struct MyStruct(u32, String);

    assert_cli_wallet_arg(
        MyStruct(1, "hello".to_string()),
        &["main", "my-struct", "1", "hello"],
    );
}
