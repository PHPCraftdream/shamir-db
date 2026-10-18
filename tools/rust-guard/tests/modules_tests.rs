//! Self-tests for the `modules` gate (CI-only): each test materialises
//! a fixture tree into a throwaway git repo and runs the built binary
//! against it, asserting the exact finding code or a clean exit.

mod harness;

use crate::harness::{assert_clean, assert_finding, TempRepo};

#[test]
fn compiler_accepted_resolution_is_clean() {
    // `#[path]`, cfg_attr path alternatives (one excepted), non-mod-rs
    // parent resolution, `include!` of a tracked file, and a raw
    // string / comment containing `mod x;` — no findings, exit 0.
    let repo = TempRepo::with_fixture("modules_ok");
    assert_clean(&repo.run_guard(&["modules"]));
}

#[test]
fn dropped_mod_declaration_is_unaccounted() {
    // `src/x_tests.rs` exists and is tracked but no `mod x_tests;`
    // anywhere — the lost-test case.
    let repo = TempRepo::with_fixture("modules_bad/dropped_mod");
    assert_finding(&repo.run_guard(&["modules"]), "UNACCOUNTED_FILE");
}

#[test]
fn unsatisfiable_cfg_is_reported() {
    let repo = TempRepo::with_fixture("modules_bad/unsat_cfg");
    assert_finding(&repo.run_guard(&["modules"]), "UNSATISFIABLE_CFG");
}

#[test]
fn missing_path_target_is_reported() {
    let repo = TempRepo::with_fixture("modules_bad/missing_path");
    assert_finding(&repo.run_guard(&["modules"]), "MISSING_MOD_FILE");
}

#[test]
fn ambiguous_resolution_is_reported() {
    // Both `dup.rs` and `dup/mod.rs` for one `mod dup;`.
    let repo = TempRepo::with_fixture("modules_bad/ambiguous");
    assert_finding(&repo.run_guard(&["modules"]), "AMBIGUOUS_MOD");
}

#[test]
fn duplicate_physical_file_is_reported() {
    // `twin.rs` reached by two logical paths via `#[path]` aliases.
    let repo = TempRepo::with_fixture("modules_bad/duplicate");
    assert_finding(&repo.run_guard(&["modules"]), "DUPLICATE_FILE");
}

#[test]
fn stale_exception_is_reported() {
    // The exception names an untracked path (`gone.rs`); the actually
    // unaccounted `ghost.rs` is additionally reported.
    let repo = TempRepo::with_fixture("modules_bad/stale_exception");
    assert_finding(&repo.run_guard(&["modules"]), "STALE_EXCEPTION");
}

#[test]
fn module_producing_macro_is_reported() {
    // A module-level `macro_rules!` whose body mentions `mod` is the
    // walk's UNRESOLVED_MACRO trigger (no [[producer]] declared); the
    // macro-generated `generated.rs` is additionally unaccounted.
    let repo = TempRepo::with_fixture("modules_bad/macro_mods");
    assert_finding(&repo.run_guard(&["modules"]), "UNRESOLVED_MACRO");
}
