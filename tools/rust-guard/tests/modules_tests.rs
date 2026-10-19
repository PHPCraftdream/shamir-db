//! Self-tests for the `modules` gate (CI-only): each test materialises
//! a fixture tree into a throwaway git repo and runs the built binary
//! against it, asserting the exact finding code or a clean exit.

mod harness;

use crate::harness::{assert_clean, assert_finding, TempRepo};

/// Writes `contents` to `rel` inside the temp repo (scan.toml,
/// hand-broken manifests).
fn write(repo: &TempRepo, rel: &str, contents: &str) {
    let path = repo.root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
    std::fs::write(path, contents).expect("write file");
}

#[test]
fn compiler_accepted_resolution_is_clean() {
    // `#[path]` (name-value attr), genuinely exclusive cfg_attr path
    // alternatives (`unix` / `not(unix)` — BOTH reachable, no exception
    // entry), BOTH branches of a lone `#[cfg_attr]` path (attr target and
    // the default `maybe.rs` fallback), an inherited `#[cfg(unix)]`
    // gating an inline module's child, a file shared across two TARGETS
    // (`benches/common.rs` auto-discovered AND `mod common;`-included),
    // a bin target (`src/bin/x.rs`, mod-free `fn main`), non-mod-rs
    // parent resolution, an inline `mod nested { pub mod leaf; }` that
    // needs NO wrapper file, `include!` of a tracked file, and a raw
    // string / comment containing `mod x;` — no findings, exit 0.
    //
    // Counterfactuals this test pins (fails without the fixes because):
    // - gitfs joined `root.join(rel.replace('/', "\\"))`, so on Linux
    //   (both CI jobs are ubuntu-latest) every repo-relative path lookup
    //   hit a literal-backslash filename and NOTHING resolved;
    // - inline mods resolved before `content` was inspected, wrongly
    //   demanding `nested.rs`/`nested/mod.rs`;
    // - the second cfg_attr alternative was dropped/excepted instead of
    //   resolved, leaving `b.rs` unaccounted (or needing an exception);
    // - `#[path = "..."]` was read with `parse_args::<LitStr>()`, which
    //   cannot extract a name-value attribute, so `aliased` missed;
    // - the default fallback branch of the lone `#[cfg_attr]` mod was
    //   never modelled, leaving `maybe.rs` unaccounted;
    // - the inherited `#[cfg(unix)]` was not AND-ed into the inline
    //   child, so `gated/leaf2.rs` carried no predicate;
    // - `benches/common.rs` was flagged DUPLICATE_FILE (duplicates are
    //   now decided WITHIN one target only);
    // - `src/bin/x.rs` was never discovered as a bin target.
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
    // `#[cfg(any())] mod ghost;` — any() with no arguments is provably
    // impossible; evaluated on the PARSED syn::Expr, so token spacing
    // cannot matter.
    let repo = TempRepo::with_fixture("modules_bad/unsat_cfg");
    assert_finding(&repo.run_guard(&["modules"]), "UNSATISFIABLE_CFG");
}

#[test]
fn missing_path_target_is_reported() {
    let repo = TempRepo::with_fixture("modules_bad/missing_path");
    assert_finding(&repo.run_guard(&["modules"]), "MISSING_MOD_FILE");
}

#[test]
fn cfg_attr_with_neither_branch_present_is_reported() {
    // A lone `#[cfg_attr(feature = "x", path = "only_x.rs")] mod m;`
    // with NEITHER `only_x.rs` nor the default `m.rs`/`m/mod.rs` on
    // disk: the attr branch resolves to Missing with fallback=false,
    // so walk.rs reports MISSING_MOD_FILE (walk.rs treats a missing
    // FALLBACK branch as silent "pred is false" by design).
    // Counterfactual: the fallback branch is silent by design, so
    // without a real attr-branch finding neither the `only_x.rs` nor
    // the `m.rs` slot would be accounted for — the missing module
    // would pass the gate unnoticed.
    let repo = TempRepo::with_fixture("modules_bad/cfg_attr_neither");
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
    // `twin.rs` reached by two UNCONDITIONAL logical paths — no cfg
    // predicates to make the edges mutually exclusive.
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
    // A module-level `macro_rules!` whose body mentions `mod` INSIDE
    // NESTED BRACES is the walk's UNRESOLVED_MACRO trigger (no
    // [[producer]] declared); the macro-generated `generated.rs` is
    // additionally unaccounted.
    // Counterfactual: fails without the descend-into-groups fix in
    // `mentions_mod`, because the top-level token trees of the
    // `macro_rules!` definition are the macro name plus one brace group
    // — no bare `mod` ident — and nothing triggered.
    let repo = TempRepo::with_fixture("modules_bad/macro_mods");
    assert_finding(&repo.run_guard(&["modules"]), "UNRESOLVED_MACRO");
}

#[test]
fn declared_producer_and_generated_file_are_clean() {
    // A documented `[[producer]]` block (name = "macro_rules",
    // packages = ["fix"]) plus an [[exception]] for the generated file:
    // fully clean run.
    // Counterfactuals this test pins (fails without the fixes because):
    // - the exceptions schema deserialized `producers` under the field
    //   name only; the documented/shipped `[[producer]]` spelling was
    //   silently dropped, so the macro went UNRESOLVED_MACRO;
    // - `mentions_mod` never descended into token groups, so the macro
    //   body never triggered the producer lookup at all.
    let repo = TempRepo::with_fixture("modules_ok/producer_declared");
    assert_clean(&repo.run_guard(&["modules"]));
}

#[test]
fn producer_scoped_to_other_package_is_not_accepted() {
    // `has_producer` must match on `packages` membership: a producer
    // declared for `some-other-package` must not silence THIS package's
    // module-producing macro.
    // Counterfactual: fails without the package-scoped lookup (the
    // name-only check accepted the mismatched producer and the macro
    // finding was suppressed).
    let repo = TempRepo::with_fixture("modules_bad/producer_wrong_package");
    assert_finding(&repo.run_guard(&["modules"]), "UNRESOLVED_MACRO");
}

#[test]
fn unparseable_manifest_is_a_finding() {
    // `tools/gadget/Cargo.toml` is hand-scanned (outside cargo
    // metadata's member list); an unparseable manifest must fail closed
    // with BAD_MANIFEST instead of being silently skipped — otherwise
    // every target under it silently vanishes from the scan.
    // Counterfactual: fails without the fix because the unreadable/
    // unparseable manifest was `return`ed on silently and the run was
    // clean.
    let repo = TempRepo::with_fixture("modules_bad/bad_manifest");
    assert_finding(&repo.run_guard(&["modules"]), "BAD_MANIFEST");
}

#[test]
fn real_layout_mini_is_fully_accounted() {
    // Miniature of the real workspace layout: workspace root manifest
    // with members = ["crates/a"], a package with plain + #[path]-mounted
    // modules, src/bin/x.rs, tests/t.rs with a #[path] helper dir,
    // benches/b.rs (harness = false) and build.rs. The fixture carries
    // its OWN root and package manifests (the harness scaffold keeps
    // existing files, so they are not clobbered).
    // Counterfactual: fails without the fix because the first CI run
    // reported every file of this layout UNACCOUNTED_FILE when root
    // manifest discovery failed.
    let repo = TempRepo::with_fixture("modules_ok/real_layout_mini");
    assert_clean(&repo.run_guard(&["modules"]));
}

#[test]
fn path_escaping_the_repo_is_reported() {
    // `#[path = "../../../outside.rs"]` climbs out of the repository.
    // Counterfactual: fails without lexical path normalisation because
    // the un-normalised spelling (`crates/fix/src/../../../outside.rs`)
    // was used as-is, the file was never found nor flagged, and the run
    // was clean apart from the missing module (or worse: the escaped
    // spelling silently became a visited key).
    let repo = TempRepo::with_fixture("modules_bad/path_escapes");
    assert_finding(&repo.run_guard(&["modules"]), "PATH_ESCAPES_REPO");
}

#[test]
fn generated_include_without_producer_is_reported() {
    // `include!(concat!(env!("OUT_DIR"), "/g.rs"))` is not statically
    // resolvable and no [[producer]] is declared.
    // Counterfactual: fails without the whole-token-stream check because
    // the inner `"/g.rs"` literal was mistaken for the entire include
    // path (resolved relative to src/, "not found" — still a finding,
    // but the produced-file branch and the producer lookup were never
    // consulted, so a scoped producer could not have silenced it).
    let repo = TempRepo::with_fixture("modules_bad/generated_include");
    assert_finding(&repo.run_guard(&["modules"]), "UNRESOLVED_INCLUDE");
}

#[test]
fn generated_include_with_scoped_producer_is_clean() {
    // The same generated include WITH a scoped `include` [[producer]]:
    // clean run. Counterfactual: fails without the whole-token-stream
    // check because the producer branch was never reached (the inner
    // literal was "resolved" and reported MISSING instead).
    let repo = TempRepo::with_fixture("modules_ok/generated_include_producer");
    assert_clean(&repo.run_guard(&["modules"]));
}

#[test]
fn scan_toml_exclusion_is_honored() {
    // `.rust-guard/scan.toml` `exclude_paths` removes a prefix from the
    // scan: the otherwise-unaccounted `extra.rs` is out of scope.
    // Counterfactual: without the exclude_paths support this run reports
    // UNACCOUNTED_FILE for the excluded file (see the sibling test that
    // pins exactly that without a scan.toml).
    let repo = TempRepo::with_fixture("modules_bad/unaccounted_extra");
    write(
        &repo,
        ".rust-guard/scan.toml",
        "exclude_paths = [\"crates/fix/src/extra.rs\"]\n",
    );
    assert_clean(&repo.run_guard(&["modules"]));
}

#[test]
fn unaccounted_extra_without_scan_toml_is_reported() {
    // The same fixture WITHOUT scan.toml: `extra.rs` is tracked but no
    // `mod extra;` anywhere — the lost-test case the exclusion above
    // must not be able to hide behind silently when undesired.
    let repo = TempRepo::with_fixture("modules_bad/unaccounted_extra");
    assert_finding(&repo.run_guard(&["modules"]), "UNACCOUNTED_FILE");
}

#[test]
fn cargo_metadata_failure_is_a_finding_and_scan_continues() {
    // `members = ["crates/nope"]` makes `cargo metadata` fail. The gate
    // must NOT die as an infra error: the failure is a
    // CARGO_METADATA_FAILED finding and the scan CONTINUES on the
    // hand-parsed manifests (here: none resolve, so the crate's files
    // are unaccounted on top).
    // Counterfactual: fails without the hand-scan fallback because
    // run_cargo_metadata's error propagated as `Err` and the run printed
    // `rust-guard: error` instead of findings.
    let repo = TempRepo::with_fixture("modules_ok");
    write(
        &repo,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/nope\"]\nresolver = \"2\"\n",
    );
    assert_finding(&repo.run_guard(&["modules"]), "CARGO_METADATA_FAILED");
}
