//! Self-tests for the `structure` gate (CI-only): one fixture repo per
//! rule, the binary invoked with `--paths **/*.rs`.

mod harness;

use crate::harness::{assert_clean, assert_finding, TempRepo};
use std::process::Output;

fn scan(repo: &TempRepo) -> Output {
    repo.run_guard(&["structure", "--paths", "**/*.rs"])
}

#[test]
fn impl_inside_defining_module_is_accepted() {
    // Negative control: the same fixture, scoped to the type's own
    // defining module subtree, is clean.
    let repo = TempRepo::with_fixture("structure/owned_impl");
    assert_clean(&repo.run_guard(&["structure", "--paths", "crates/fix/src/repo/**"]));
}

#[test]
fn logic_in_mod_rs_is_reported() {
    let repo = TempRepo::with_fixture("structure/modrs_logic");
    assert_finding(&scan(&repo), "MODRS_LOGIC");
}

#[test]
fn use_in_tests_manifest_is_reported() {
    let repo = TempRepo::with_fixture("structure/tests_manifest");
    assert_finding(&scan(&repo), "TESTS_MANIFEST");
}

#[test]
fn inline_test_mod_is_reported() {
    let repo = TempRepo::with_fixture("structure/inline_test_mod");
    assert_finding(&scan(&repo), "INLINE_TEST_MOD");
}

#[test]
fn function_local_use_is_reported() {
    let repo = TempRepo::with_fixture("structure/local_use");
    assert_finding(&scan(&repo), "LOCAL_USE");
}

#[test]
fn impl_outside_defining_module_is_reported() {
    // `.rust-guard/owned_impl.toml` in the fixture copy is picked up
    // because cwd is the temp repo root.
    let repo = TempRepo::with_fixture("structure/owned_impl");
    assert_finding(&scan(&repo), "OWNED_IMPL");
}
