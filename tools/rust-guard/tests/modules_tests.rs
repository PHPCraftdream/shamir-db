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
    // `#[path]`, plain inherited cfg propagation; path normalization; and
    // a file shared across two TARGETS
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
    // - `#[path = "..."]` was read with `parse_args::<LitStr>()`, which
    //   cannot extract a name-value attribute, so `aliased` missed;
    // - `#[path = "./sub/../sub/positive.rs"]` normalises before its
    //   repository-relative visited key is recorded. If lookup used the raw
    //   spelling while the tree used the normalised key, the run reported
    //   `UNACCOUNTED_FILE` for `sub/positive.rs`.
    // - the inherited `#[cfg(unix)]` was not AND-ed into the inline
    //   module's file-backed child, so `gated/leaf2.rs`'s two edges
    //   (`all(unix)` vs `not(unix)`) were not mutually exclusive and
    //   the run reported DUPLICATE_FILE;
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
fn cfg_attr_path_is_rejected() {
    // Counterfactual: without structured classification this would only be
    // reported as MISSING_MOD_FILE or treated as a normal default module.
    let repo = TempRepo::with_fixture("modules_bad/cfg_attr_neither");
    let output = repo.run_guard(&["modules"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_finding(&output, "UNSUPPORTED_CFG_ATTR_PATH");
    assert_eq!(
        stderr.matches("UNSUPPORTED_CFG_ATTR_PATH").count(),
        1,
        "expected one structured rejection: {stderr}"
    );
    assert!(!stderr.contains("MISSING_MOD_FILE"), "{stderr}");
}

#[test]
fn multiple_cfg_attr_paths_are_rejected() {
    // First-only detectors miss the second path-bearing attribute.
    let repo = TempRepo::with_fixture("modules_bad/cfg_attr_neither_two");
    let output = repo.run_guard(&["modules"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_finding(&output, "UNSUPPORTED_CFG_ATTR_PATH");
    assert_eq!(
        stderr.matches("UNSUPPORTED_CFG_ATTR_PATH").count(),
        1,
        "{stderr}"
    );
    assert!(!stderr.contains("MISSING_MOD_FILE"), "{stderr}");
}

#[test]
fn nested_cfg_attr_path_is_rejected() {
    // A nonrecursive detector misses the inner path argument. The nested
    // variant is written over the fixture lib.rs in-process (no extra
    // fixture tree), then rejected end-to-end.
    let repo = TempRepo::with_fixture("modules_bad/cfg_attr_neither");
    std::fs::write(
        repo.root.join("crates/fix/src/lib.rs"),
        "#[cfg_attr(feature = \"a\", cfg_attr(feature = \"b\", path = \"gen.rs\"))]\nmod m;\n",
    )
    .expect("rewrite fixture lib.rs");
    let output = repo.run_guard(&["modules"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_finding(&output, "UNSUPPORTED_CFG_ATTR_PATH");
    assert_eq!(
        stderr.matches("UNSUPPORTED_CFG_ATTR_PATH").count(),
        1,
        "expected exactly one structured rejection: {stderr}"
    );
    assert!(!stderr.contains("MISSING_MOD_FILE"), "{stderr}");
}

#[test]
fn ambiguous_resolution_is_reported() {
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
    // its OWN root and package manifests, so the harness scaffold adds
    // NOTHING (a fixture shipping a root manifest owns its whole member
    // list — finding 4).
    // Counterfactuals this test pins (fails without the fixes because):
    // - the first CI run reported every file of this layout
    //   UNACCOUNTED_FILE when root manifest discovery failed;
    // - the scaffold injected an undeclared `crates/fix` package that
    //   `cargo metadata` excluded from the workspace members while its
    //   lib.rs stayed in the accounting scope — UNACCOUNTED_FILE for
    //   crates/fix/src/lib.rs.
    let repo = TempRepo::with_fixture("modules_ok/real_layout_mini");
    assert_clean(&repo.run_guard(&["modules"]));
}

#[test]
fn path_escaping_the_repo_is_reported() {
    // `#[path = "../../../../outside.rs"]` pops PAST the repo root:
    // from crates/fix/src, THREE `..` reach exactly the repo root (a
    // repo-root-level `outside.rs`, which would resolve — as Missing —
    // and is NOT an escape); only FOUR pop beyond the top.
    // Counterfactual: fails without lexical path normalisation because
    // the un-normalised spelling (`crates/fix/src/../../../../outside.rs`)
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
fn scan_toml_exclusion_covers_manifest_discovery() {
    // Finding 1: an excluded prefix must remove a manifest from
    // DISCOVERY, not just from the walk/inventory scope. The fixture's
    // `tools/gadget/Cargo.toml` is deliberately unparseable and lives
    // under the excluded `tools/gadget` prefix; the run must be CLEAN.
    // Counterfactual: without discovery-side exclusion the manifest was
    // still enumerated and parsed, and the run reported BAD_MANIFEST
    // for tools/gadget/Cargo.toml.
    let repo = TempRepo::with_fixture("modules_bad/bad_manifest");
    write(
        &repo,
        ".rust-guard/scan.toml",
        "exclude_paths = [\"tools/gadget\"]\n",
    );
    assert_clean(&repo.run_guard(&["modules"]));
}

#[test]
fn shared_child_double_mount_within_one_target_is_reported() {
    // Finding 6: children of a shared file must be traversed once per
    // (target, file), not once per physical file globally. bench_b
    // mounts common/helper.rs AND common/helper/child.rs directly, so
    // within TARGET bench_b child.rs has two conflicting logical paths.
    // Counterfactual: pre-fix bench_a traversed helper.rs first (target
    // bench_a), so bench_b only recorded the helper edge without
    // traversing it — the `helper::child` edge never existed for target
    // bench_b, only `c2` did, and no duplicate was reported.
    let repo = TempRepo::with_fixture("modules_bad/shared_child_dup");
    assert_finding(&repo.run_guard(&["modules"]), "DUPLICATE_FILE");
}

#[test]
fn stale_exception_for_single_reachability_is_reported() {
    // Finding 10: an exception is consumed only when it actually
    // suppresses a duplicate. `once.rs` is mounted exactly once (no
    // conflict anywhere), so its exception is stale.
    // Counterfactual: pre-fix check_duplicates marked ANY visited
    // path's exception used, so the reachable-without-conflict file's
    // exception was consumed, the run was CLEAN and the stale entry
    // hid forever.
    let repo = TempRepo::with_fixture("modules_bad/stale_exception_single");
    assert_finding(&repo.run_guard(&["modules"]), "STALE_EXCEPTION");
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
    // hand-scanned manifests, including the malformed gadget manifest.
    // Counterfactual: fails without the hand-scan fallback because
    // run_cargo_metadata's error propagated as `Err` and the run printed
    // `rust-guard: error` instead of findings.
    let repo = TempRepo::with_fixture("modules_bad/bad_manifest");
    write(
        &repo,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/nope\"]\nresolver = \"2\"\n",
    );
    let output = repo.run_guard(&["modules"]);
    assert_finding(&output, "CARGO_METADATA_FAILED");
    assert_finding(&output, "BAD_MANIFEST");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("tools/gadget/Cargo.toml"),
        "expected malformed manifest path: {stderr}"
    );
    assert!(
        !stderr.contains("rust-guard: error"),
        "infra abort: {stderr}"
    );
}

#[test]
fn same_source_exclusive_mounts_still_walk_children_for_duplicate_detection() {
    // Counterfactual: if the walk were cached only by physical file, the
    // `b` mount would not visit shared.rs children. Actual trace: lib.rs:1
    // reaches shared.rs under `a`/unix, :4 reaches it under `b`/not(unix);
    // shared.rs:1 mounts shared/child.rs under each composed cfg; lib.rs:7
    // mounts that same child directly under not(unix). a::child/direct are
    // exclusive but b::child/direct conflict, proving the second logical mount.
    let repo = TempRepo::with_fixture("modules_bad/logical_child_dup");
    let output = repo.run_guard(&["modules"]);
    assert_finding(&output, "DUPLICATE_FILE");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("child.rs"),
        "expected child-file conflict: {stderr}"
    );
}

#[test]
fn exclusive_shared_source_mounts_are_clean_without_child_conflict() {
    // Counterfactual: without composed exclusive-cfg handling, the two
    // logical mounts of shared.rs itself would look like DUPLICATE_FILE.
    // Final trace: lib.rs:1/3 mount shared.rs as a on unix and b on not(unix);
    // shared.rs:1 mounts shared/leaf.rs under those same composed predicates.
    // Each physical file therefore has only pairwise-exclusive reaching edges.
    let repo = TempRepo::with_fixture("modules_ok/exclusive_shared");
    assert_clean(&repo.run_guard(&["modules"]));
}

#[test]
fn recursive_path_mount_reports_mount_cycle() {
    // Counterfactual: without ancestor checking, the file-backed mod edge
    // would recursively revisit lib.rs without bound. The path in this fixture
    // targets exactly the root ancestor, which emits MOUNT_CYCLE at lib.rs:1.
    let repo = TempRepo::with_fixture("modules_bad/mount_cycle");
    let output = repo.run_guard(&["modules"]);
    assert_finding(&output, "MOUNT_CYCLE");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("crates/fix/src/lib.rs"),
        "cycle must identify lib.rs: {stderr}"
    );
    assert!(
        !stderr.contains("rust-guard: error"),
        "infra error: {stderr}"
    );
    assert_eq!(
        stderr.matches("MOUNT_CYCLE").count(),
        1,
        "expected one cycle finding: {stderr}"
    );
}
