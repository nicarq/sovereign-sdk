fn set_constants_manifest() {
    std::env::set_var("CONSTANTS_MANIFEST_TEST_MODE", "1");
}

#[test]
fn module_info_tests() {
    set_constants_manifest();
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/trybuild/module_info/derive_on_enum_not_supported.rs");
    t.compile_fail("tests/integration/trybuild/module_info/field_missing_attribute.rs");
    t.compile_fail("tests/integration/trybuild/module_info/missing_address.rs");
    t.compile_fail("tests/integration/trybuild/module_info/no_generics.rs");
    t.compile_fail("tests/integration/trybuild/module_info/not_supported_type.rs");
    t.compile_fail("tests/integration/trybuild/module_info/second_addr_not_supported.rs");
}

#[test]
fn module_dispatch_tests() {
    set_constants_manifest();
    let t = trybuild::TestCases::new();
    t.pass("tests/integration/trybuild/dispatch/derive_genesis.rs");
    t.pass("tests/integration/trybuild/dispatch/derive_dispatch.rs");
    t.pass("tests/integration/trybuild/dispatch/derive_event.rs");
    t.pass("tests/integration/trybuild/dispatch/derive_dispatch_custom_attrs.rs");
    t.compile_fail("tests/integration/trybuild/dispatch/derive_event_no_default_attrs.rs");
}

#[test]
fn rpc_tests() {
    set_constants_manifest();
    let t = trybuild::TestCases::new();
    t.pass("tests/integration/trybuild/rpc/derive_rpc.rs");
    t.pass("tests/integration/trybuild/rpc/expose_rpc.rs");
    t.pass("tests/integration/trybuild/rpc/expose_rpc_associated_types.rs");
    t.pass("tests/integration/trybuild/rpc/expose_rpc_associated_types_nested.rs");

    t.compile_fail("tests/integration/trybuild/rpc/derive_rpc_working_set_immutable_reference.rs");
    t.compile_fail("tests/integration/trybuild/rpc/derive_rpc_working_set_no_generic.rs");
    t.compile_fail("tests/integration/trybuild/rpc/expose_rpc_associated_type_not_static.rs");
    t.compile_fail("tests/integration/trybuild/rpc/expose_rpc_first_generic_not_spec.rs");
}

#[test]
fn rest_api_tests() {
    set_constants_manifest();
    let t = trybuild::TestCases::new();

    t.pass("tests/integration/trybuild/rest/derive_rest_api.rs");
}

#[test]
fn constants_from_manifests_test() {
    set_constants_manifest();
    let t: trybuild::TestCases = trybuild::TestCases::new();

    // TODO: Add compile fail on address prefix and mismatched prefix and invalid bech3
    t.pass("tests/integration/trybuild/constants/valid_constants.rs");
    t.compile_fail("tests/integration/trybuild/constants/bech32_constant_invalid_checksum.rs");
    t.compile_fail("tests/integration/trybuild/constants/bech32_constant_not_a_string.rs");
    t.compile_fail("tests/integration/trybuild/constants/bech32_constant_prefix_too_short.rs");
    t.compile_fail("tests/integration/trybuild/constants/bech32_constant_prefix_too_long.rs");
}
