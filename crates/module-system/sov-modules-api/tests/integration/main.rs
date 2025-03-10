mod proof_tests;
mod rest_api;
mod state_tests;
mod transaction;

#[test]
fn trybuild() {
    let t = trybuild::TestCases::new();

    t.compile_fail("tests/integration/state_tests/trybuild/state_cannot_mutate_while_borrowed.rs");
    t.compile_fail("tests/integration/state_tests/trybuild/state_cannot_borrow_mut_twice.rs");
    t.compile_fail(
        "tests/integration/state_tests/trybuild/state_cannot_borrow_while_borrowed_mut.rs",
    );
}
