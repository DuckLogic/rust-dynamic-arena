
#[test]
fn compile_test() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/compile-fail/invalid_drop_counted.rs");
}
