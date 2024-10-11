use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_universal_wallet::schema::{IndexLinking, Item, Link, Primitive, Schema, SchemaGenerator};
use sov_universal_wallet::UniversalWallet;

// Hack - because the macro is configured to be re-exported from sov_rollup_interface;
// but _we_ are a dependency of sov_rollup_interface so we can't import it without causing a cycle
// This should not be an issue anywhere else except inside this crate's tests right here
mod sov_rollup_interface {
    pub use sov_universal_wallet;
}

// TODO: there's probably a better way than nested macros
macro_rules! encode_decode_tests_simple {
    ($schema:ident, $item:ident, $expected_display:literal) => {
        let borsh_ser = borsh::to_vec(&$item).unwrap();
        let json = serde_json::to_string(&$item).unwrap();
        assert_eq!($schema.display(&borsh_ser).unwrap(), $expected_display);
        // println!("JSON: {json}");
        assert_eq!($schema.json_to_borsh(&json).unwrap(), borsh_ser);
    };
}

macro_rules! encode_decode_tests {
    ($schema:ident, $item:ident, $expected_display:literal) => {
        // println!("{:?}", &$schema);
        encode_decode_tests_simple!($schema, $item, $expected_display);
        let schema_json = serde_json::to_string(&$schema).unwrap();
        let recovered_schema = Schema::from_json(&schema_json).unwrap();
        encode_decode_tests_simple!(recovered_schema, $item, $expected_display);
    };
}

pub trait Spec {
    type Address: SchemaGenerator;
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub enum EnumWithAssociatedType<S: Spec> {
    AssociatedVariant { address: S::Address },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub enum EnumWithWhereClauseAssociatedType<S>
where
    S: Spec,
{
    TheVariant { address: S::Address },
}

#[test]
fn test_associated_types() {
    struct MySpec();
    impl Spec for MySpec {
        type Address = u64;
    }

    let schema = Schema::of::<EnumWithAssociatedType<MySpec>>();
    let my_enum = EnumWithAssociatedType::<MySpec>::AssociatedVariant { address: 123 };

    encode_decode_tests!(schema, my_enum, "AssociatedVariant { address: 123 }");

    let schema = Schema::of::<EnumWithWhereClauseAssociatedType<MySpec>>();
    let my_enum = EnumWithWhereClauseAssociatedType::<MySpec>::TheVariant { address: 123 };

    encode_decode_tests!(schema, my_enum, "TheVariant { address: 123 }");
}

#[test]
fn test_inner_item_derive() {
    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
    #[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
    struct A {
        my_field: u8,
    }

    let schema = Schema::of::<A>();
    let my_a = A { my_field: 32 };
    encode_decode_tests!(schema, my_a, "{ my_field: 32 }");
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize)]
/// A type which doesn't derive `UniversalWallet` and doesn't have a schema gen implementation
pub struct NoSchemaU64Wrapper(pub u64);

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
#[serde(rename_all = "snake_case")]
pub enum TestCall {
    Register(Registration),
    RegisterMany(Vec<RegistrationLike>),
    RegisterSimple(Vec<u8>),
    Withdraw(u64),
    #[serde(skip)]
    #[allow(unused)]
    // We need this variant to test the schema generation of recursive types even though we don't construct it
    Complex(#[cfg_attr(test, sov_wallet(bound = ""))] Box<Complex>),
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub struct MinimalStruct {
    tokens: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
#[serde(rename_all = "snake_case")]
pub struct Registration {
    address: [u8; 32],
    role: Role,
    tokens: u64,
    #[cfg_attr(test, sov_wallet(as_ty = "u64"))]
    schemaless: NoSchemaU64Wrapper,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub struct StringWrapper(pub String);

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(BorshSerialize, BorshDeserialize))]
pub struct SchemalessStringWrapper(pub String);

#[cfg(test)]
impl SchemaGenerator for SchemalessStringWrapper {
    fn scaffold() -> Item<IndexLinking> {
        Item::Atom(Primitive::String)
    }
    fn get_child_links(_schema: &mut Schema) -> Vec<Link> {
        Vec::new()
    }
}

#[derive(Debug, PartialEq, Eq, Clone, UniversalWallet)]
#[cfg_attr(test, derive(BorshSerialize, BorshDeserialize))]
pub struct VecOfThing {
    memo: Vec<StringWrapper>,
}

#[derive(Debug, PartialEq, Eq, Clone, UniversalWallet)]
#[cfg_attr(test, derive(BorshSerialize, BorshDeserialize))]
pub struct VecOfWrapper {
    memo: Vec<SchemalessStringWrapper>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub struct RegistrationLike {
    address: [u8; 32],
    some_bytes: Vec<u8>,
    extra_complexity: Vec<AThirdComplexType>,
}

type VecAlias = Vec<u8>;
type IntAlias = i32;
type TestCallAlias = TestCall;

#[derive(Debug, PartialEq, Serialize, Eq, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize))]
#[serde(rename_all = "snake_case")]
pub enum TestCallRec<T> {
    Withdraw(Box<T>),
    // #[serde(skip)]
    #[allow(unused)]
    // We need this variant to test the schema generation of recursive types even though we don't construct it
    Complex(#[sov_wallet(bound = "Box<T>: SchemaGenerator")] Box<ComplexRec<T>>),
    // Complex(Box<ComplexRec<T>>),
}

#[derive(Debug, PartialEq, Serialize, Eq, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize))]
#[serde(rename_all = "snake_case")]
pub enum TestCallStructRec<T> {
    Withdraw(Box<T>),
    #[allow(unused)]
    Complex {
        #[sov_wallet(bound = "Box<T>: SchemaGenerator")]
        rec_field: Box<ComplexRec<T>>,
    },
}

#[derive(Debug, PartialEq, Serialize, Eq, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize))]
pub struct ComplexRec<T> {
    unnested_enum: TestCallRec<T>,
}

#[test]
fn test_tuple_schema_recursive_generic() {
    let schema = Schema::of::<TestCallRec<u64>>();
    let my_call = TestCallRec::<u64>::Withdraw(Box::new(43));

    encode_decode_tests!(schema, my_call, "Withdraw(43)");
}

#[test]
fn test_tuple_struct_schema_recursive_generic() {
    let schema = Schema::of::<TestCallStructRec<u64>>();
    let my_call = TestCallStructRec::<u64>::Withdraw(Box::new(43));

    encode_decode_tests!(schema, my_call, "Withdraw(43)");
}

#[derive(Debug, PartialEq, Serialize, Eq, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize))]
pub enum TestCallRecNonGeneric {
    Withdraw(u64),
    // #[serde(skip)]
    #[allow(unused)]
    // We need this variant to test the schema generation of recursive types even though we don't construct it
    Complex(#[sov_wallet(bound = "")] Box<ComplexRecNonGeneric>),
}
#[derive(Debug, PartialEq, Serialize, Eq, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize))]
pub struct ComplexRecNonGeneric {
    unnested_enum: TestCallRecNonGeneric,
}
#[test]
fn test_medium_struct_schema_recursive_nongeneric() {
    let schema = Schema::of::<TestCallRecNonGeneric>();
    let my_call = TestCallRecNonGeneric::Withdraw(43);

    encode_decode_tests!(schema, my_call, "Withdraw(43)");
}

const PREFIX_SOV: &str = "sov";
const PREFIX_CELESTIA: &str = "celestia";

#[derive(Debug, PartialEq, Serialize, Eq, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub struct Complex {
    #[cfg_attr(test, sov_wallet(display(bech32(prefix = "PREFIX_SOV"))))]
    rollup_address: [u8; 32],
    #[cfg_attr(
        test,
        sov_wallet(display(bech32m(prefix = "PREFIX_CELESTIA"))),
        serde(with = "serde_arrays")
    )]
    da_address: [u8; 64],
    #[cfg_attr(test, sov_wallet(display(decimal)))]
    message: Vec<u8>,
    // Important: we cover the case of...
    // - an aliased field, followed by a field with a different schema, followed by a Vec containing the aliased type
    // using SchemalessStringWrapper. Be careful about reording and/or deleting the aliased field here, since
    // we can have regressions on that edge case.
    first_item: SchemalessStringWrapper,
    role: Role,
    tokens: u64,
    memo: Vec<SchemalessStringWrapper>,
    registration: Registration,
    events: Vec<Vec<u8>>,
    child_addresses: Vec<[u8; 32]>,
    nested_vec_bytes: Vec<Vec<Vec<u8>>>,
    nested_vec_enum: Vec<Vec<TestCall>>,
    aliased_vec: VecAlias,
    aliased_int: IntAlias,
    aliased_call: TestCallAlias,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub struct Generic<T> {
    contents: T,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub struct NestedGeneric<T> {
    contents: Generic<T>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub struct AThirdComplexType {
    #[cfg_attr(test, sov_wallet(display(bech32(prefix = "PREFIX_CELESTIA"))))]
    address: [u8; 32],
    #[cfg_attr(test, sov_wallet(display(decimal)))]
    extra_bytes: Vec<u8>,
    role: Role,
}

#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    Serialize,
    Deserialize,
    BorshDeserialize,
    BorshSerialize,
    UniversalWallet,
)]
pub enum Role {
    Attester,
    Challenger,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
#[serde(rename_all = "snake_case", rename = "a")]
pub enum RuntimeCall {
    TestCall(TestCall),
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub enum EnumWithStruct {
    Foo {
        first_field: u64,
        second_field: String,
        third_field: u32,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub enum EnumWithTwoStructs {
    Foo {
        first_field: u64,
        second_field: String,
        third_field: u32,
    },
    Bar {
        first_field: u64,
        second_field: u8,
        third_field: u32,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub enum EnumWithStructAndGeneric<T> {
    Foo {
        first_field: u64,
        second_field: Generic<T>,
        third_field: u32,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub enum EnumWithStructAndThreeGenerics<T, U, V> {
    VarA {
        first_field: u64,
        second_field: Generic<T>,
        third_field: u32,
    },
    VarB {
        first_field: u64,
        second_field: Generic<U>,
        third_field: u32,
    },
    VarC {
        first_field: u64,
        second_field: Generic<Generic<V>>,
        third_field: u32,
    },
    VarD,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub enum EnumWithMultiTupleSimple {
    TheVariant(u64, EnumWithStruct, String),
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub enum EnumWithMultiTuple {
    TheVariant(u64, EnumWithStruct, RuntimeCall),
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub enum EnumWithMultiTupleAndGenerics<T, U, V> {
    One(u64, EnumWithStructAndThreeGenerics<T, U, V>, String),
    Two(Generic<U>),
    Three,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize, BorshDeserialize))]
pub enum EnumWithIdenticalTuples {
    FirstVariant(u64, EnumWithStruct, String),
    SecondVariant(u64, EnumWithStruct, String),
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize))]
pub struct WithSilentField<T> {
    int: u64,
    #[cfg_attr(test, sov_wallet(hidden))]
    skipped: T,
    str: &'static str,
    phantom: PhantomData<u64>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(UniversalWallet, BorshSerialize))]
struct WithTuples {
    #[allow(unused_parens)]
    double: (u64, u64),
    mixed: (u64, String, Role),
    quintuple: (u64, u64, u64, u64, u64),
    octuple: (u64, u64, u64, u64, u64, u64, u64, u64),
}

#[test]
fn test_tuples() {
    let schema = Schema::of::<WithTuples>();

    let my_with_tuples = WithTuples {
        double: (5, 8),
        mixed: (13214, "hello".to_string(), Role::Challenger),
        quintuple: (1, 2, 3, 4, 5),
        octuple: (1, 2, 3, 4, 5, 6, 7, 8),
    };

    encode_decode_tests!(schema, my_with_tuples, "{ double: (5, 8), mixed: (13214, \"hello\", RoleChallenger), quintuple: (1, 2, 3, 4, 5), octuple: (1, 2, 3, 4, 5, 6, 7, 8) }");
}

#[test]
fn test_enum_with_complex_tuples() {
    let schema = Schema::of::<EnumWithMultiTuple>();
    let runtime_call = RuntimeCall::TestCall(TestCall::Register(Registration {
        address: [17; 32],
        role: Role::Attester,
        tokens: 1000,
        schemaless: NoSchemaU64Wrapper(123),
    }));

    let my_enum = EnumWithMultiTuple::TheVariant(
        16,
        EnumWithStruct::Foo {
            first_field: 84,
            second_field: "abcd".to_string(),
            third_field: 14,
        },
        runtime_call,
    );

    encode_decode_tests!(schema, my_enum,
        "TheVariant(16, Foo { first_field: 84, second_field: \"abcd\", third_field: 14 }, TestCall.Register { address: 0x1111111111111111111111111111111111111111111111111111111111111111, role: Attester, tokens: 1000, schemaless: 123 })"
    );
}

#[test]
fn test_enum_with_simple_tuples() {
    let schema = Schema::of::<EnumWithMultiTupleSimple>();

    let my_enum = EnumWithMultiTupleSimple::TheVariant(
        16,
        EnumWithStruct::Foo {
            first_field: 84,
            second_field: "abcd".to_string(),
            third_field: 14,
        },
        "hello".to_string(),
    );

    encode_decode_tests!(schema, my_enum,
        "TheVariant(16, Foo { first_field: 84, second_field: \"abcd\", third_field: 14 }, \"hello\")"
    );
}

#[test]
fn test_enum_with_identical_tuples() {
    let schema = Schema::of::<EnumWithIdenticalTuples>();

    let my_enum = EnumWithIdenticalTuples::SecondVariant(
        16,
        EnumWithStruct::Foo {
            first_field: 84,
            second_field: "abcd".to_string(),
            third_field: 14,
        },
        "hello".to_string(),
    );

    encode_decode_tests!(schema, my_enum,
        "SecondVariant(16, Foo { first_field: 84, second_field: \"abcd\", third_field: 14 }, \"hello\")"
    );
}

#[test]
fn test_enum_with_struct() {
    let schema = Schema::of::<EnumWithStruct>();

    let my_with_tuples = EnumWithStruct::Foo {
        first_field: 84,
        second_field: "abcd".to_string(),
        third_field: 14,
    };

    encode_decode_tests!(
        schema,
        my_with_tuples,
        "Foo { first_field: 84, second_field: \"abcd\", third_field: 14 }"
    );
}

#[test]
fn test_enum_with_struct_and_generic() {
    let schema = Schema::of::<EnumWithStructAndGeneric<u32>>();

    let my_with_tuples = EnumWithStructAndGeneric::Foo {
        first_field: 84,
        second_field: Generic { contents: 52 },
        third_field: 14,
    };

    encode_decode_tests!(
        schema,
        my_with_tuples,
        "Foo { first_field: 84, second_field: { contents: 52 }, third_field: 14 }"
    );
}

#[test]
fn test_enum_with_struct_and_multiple_generics() {
    let schema = Schema::of::<EnumWithStructAndThreeGenerics<u32, u8, i8>>();

    let my_with_generics: EnumWithStructAndThreeGenerics<u32, u8, i8> =
        EnumWithStructAndThreeGenerics::VarA {
            first_field: 84,
            second_field: Generic {
                contents: 2_000_000_000,
            },
            third_field: 14,
        };

    encode_decode_tests!(
        schema,
        my_with_generics,
        "VarA { first_field: 84, second_field: { contents: 2000000000 }, third_field: 14 }"
    );
}

#[test]
fn test_enum_with_tuples_and_generics() {
    let schema = Schema::of::<EnumWithMultiTupleAndGenerics<u32, u8, i8>>();

    let my_var_two: EnumWithMultiTupleAndGenerics<u32, u8, i8> =
        EnumWithMultiTupleAndGenerics::Two(Generic { contents: 19 });
    let my_var_one: EnumWithMultiTupleAndGenerics<u32, u8, i8> = EnumWithMultiTupleAndGenerics::One(
        1245345,
        EnumWithStructAndThreeGenerics::VarC {
            first_field: 435653,
            second_field: Generic {
                contents: Generic { contents: -5 },
            },
            third_field: 73242,
        },
        "abdsf".to_string(),
    );

    encode_decode_tests!(schema, my_var_two, "Two { contents: 19 }");

    encode_decode_tests!(schema, my_var_one,
        "One(1245345, VarC { first_field: 435653, second_field: { contents: { contents: -5 } }, third_field: 73242 }, \"abdsf\")"
        );
}

#[test]
fn test_minimal_enum_schema() {
    let schema = Schema::of::<Role>();

    let item = Role::Attester;
    encode_decode_tests!(schema, item, "Attester");
}

#[test]
fn test_minimal_struct_schema() {
    let schema = Schema::of::<MinimalStruct>();
    let my_registration = MinimalStruct { tokens: 1000 };

    encode_decode_tests!(schema, my_registration, "{ tokens: 1000 }");
}

#[test]
fn test_simple_struct_schema() {
    let schema = Schema::of::<Registration>();
    let my_registration = Registration {
        address: [17; 32],
        role: Role::Attester,
        tokens: 1000,
        schemaless: NoSchemaU64Wrapper(123),
    };

    encode_decode_tests!(schema, my_registration, "{ address: 0x1111111111111111111111111111111111111111111111111111111111111111, role: Attester, tokens: 1000, schemaless: 123 }");
}

#[test]
fn test_medium_struct_schema() {
    let schema = Schema::of::<RuntimeCall>();
    let my_call = RuntimeCall::TestCall(TestCall::Register(Registration {
        address: [17; 32],
        role: Role::Attester,
        tokens: 1000,
        schemaless: NoSchemaU64Wrapper(123),
    }));

    encode_decode_tests!(schema, my_call, "TestCall.Register { address: 0x1111111111111111111111111111111111111111111111111111111111111111, role: Attester, tokens: 1000, schemaless: 123 }");
}

#[test]
fn test_vec_schema() {
    let schema = Schema::of::<RuntimeCall>();
    let my_call = RuntimeCall::TestCall(TestCall::RegisterMany(vec![RegistrationLike {
        address: [23; 32],
        some_bytes: vec![1, 2, 3, 4, 5],
        extra_complexity: vec![AThirdComplexType {
            address: [17; 32],
            extra_bytes: vec![6, 7, 8, 9, 10],
            role: Role::Attester,
        }],
    }]));

    encode_decode_tests!(schema, my_call,
        "TestCall.RegisterMany [{ address: 0x1717171717171717171717171717171717171717171717171717171717171717, some_bytes: 0x0102030405, extra_complexity: [{ address: celestia1zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygsr0ealj, extra_bytes: [6, 7, 8, 9, 10], role: Attester }] }]"
    );
}

#[test]
fn test_vec_simple_schema() {
    let schema = Schema::of::<RuntimeCall>();
    let my_call = RuntimeCall::TestCall(TestCall::RegisterSimple(vec![1, 2, 3]));

    encode_decode_tests!(schema, my_call, "TestCall.RegisterSimple(0x010203)");
}

#[test]
fn test_vec_string() {
    let schema = Schema::of::<Vec<StringWrapper>>();
    let my_call = vec![
        StringWrapper("hello".to_string()),
        StringWrapper("world".to_string()),
    ];

    encode_decode_tests!(schema, my_call, r#"["hello", "world"]"#);
}

#[test]
fn test_complex_type() {
    let schema = Schema::of::<Complex>();
    let my_call = Complex {
        rollup_address: [1; 32],
        da_address: [2; 64],
        message: vec![3, 2, 1],
        first_item: SchemalessStringWrapper("hello".to_string()),
        role: Role::Attester,
        tokens: 1000,
        memo: vec![SchemalessStringWrapper("This is a memo".to_string())],
        registration: Registration {
            address: [17; 32],
            role: Role::Attester,
            tokens: 1000,
            schemaless: NoSchemaU64Wrapper(123),
        },
        events: vec![b"hello".to_vec(), b"goodbye".to_vec()],
        child_addresses: vec![[3; 32], [4; 32]],
        nested_vec_bytes: vec![vec![vec![0, 1, 2]]],
        nested_vec_enum: vec![vec![TestCall::Withdraw(1)], vec![TestCall::Withdraw(2)]],
        aliased_vec: vec![7, 8, 9],
        aliased_int: 1234567,
        aliased_call: TestCall::Register(Registration {
            address: [17; 32],
            role: Role::Attester,
            tokens: 1000,
            schemaless: NoSchemaU64Wrapper(123),
        }),
    };

    encode_decode_tests!(schema, my_call,
        "{ rollup_address: sov1qyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqslg48nn, da_address: celestia1qgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqs08sssm, message: [3, 2, 1], first_item: \"hello\", role: Attester, tokens: 1000, memo: [\"This is a memo\"], registration: { address: 0x1111111111111111111111111111111111111111111111111111111111111111, role: Attester, tokens: 1000, schemaless: 123 }, events: [0x68656c6c6f, 0x676f6f64627965], child_addresses: [0x0303030303030303030303030303030303030303030303030303030303030303, 0x0404040404040404040404040404040404040404040404040404040404040404], nested_vec_bytes: [[0x000102]], nested_vec_enum: [[TestCall.Withdraw(1)], [TestCall.Withdraw(2)]], aliased_vec: 0x070809, aliased_int: 1234567, aliased_call: Register { address: 0x1111111111111111111111111111111111111111111111111111111111111111, role: Attester, tokens: 1000, schemaless: 123 } }"
    );
}

#[test]
fn test_silent_simple_field() {
    let schema = Schema::of::<WithSilentField<&'static str>>();
    let my_call = WithSilentField {
        int: 123,
        skipped: "this should be skipped",
        str: "this should be included",
        phantom: Default::default(),
    };

    encode_decode_tests!(
        schema,
        my_call,
        "{ int: 123, str: \"this should be included\" }"
    );
}
// TODO: Are Vec<Box<T>> Primitive when T: Primitive? I think so.
// What about Vec<Box<u8>>? Is that a bytevec? I think so - but how do we resolve it?

#[test]
fn test_primtive_indirection() {
    let schema = Schema::of::<Generic<Box<Vec<u8>>>>();
    let generic: Generic<Box<Vec<u8>>> = Generic {
        contents: Box::new(vec![12, 34]),
    };

    encode_decode_tests!(schema, generic, "{ contents: 0x0c22 }");
}

#[test]
fn test_nested_generics() {
    let schema = Schema::of::<NestedGeneric<Vec<u8>>>();
    let generic: NestedGeneric<Vec<u8>> = NestedGeneric {
        contents: Generic {
            contents: vec![8, 34],
        },
    };

    encode_decode_tests!(schema, generic, "{ contents: { contents: 0x0822 } }");
}

#[test]
fn test_nested_silent_fields() {
    let schema = Schema::of::<WithSilentField<WithSilentField<i32>>>();
    let my_call = WithSilentField {
        int: 123,
        skipped: WithSilentField {
            int: 456,
            skipped: 789,
            str: "hi",
            phantom: Default::default(),
        },
        phantom: PhantomData,
        str: "this should be included",
    };

    encode_decode_tests!(
        schema,
        my_call,
        "{ int: 123, str: \"this should be included\" }"
    );
}
