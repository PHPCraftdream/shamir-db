//! Self-tests for the `structure` gate (CI-only): one fixture repo per
//! rule, the binary invoked with `--paths **/*.rs`.

mod harness;

use crate::harness::{assert_clean, assert_finding, copy_fixture, TempRepo};
use std::process::Output;

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn scan(repo: &TempRepo) -> Output {
    repo.run_guard(&["structure", "--paths", "**/*.rs"])
}

/// A two-commit repo plus the first commit's SHA, for the diff-based
/// rules (VIS_WIDENING / ALLOW_ADDED). Local to this file, following
/// the same pattern as `ConsRepo` in conservation_tests.rs.
struct DiffRepo {
    repo: TempRepo,
    base_sha: String,
}

impl DiffRepo {
    /// `fixtures/<fixture>/base` is committed first, then
    /// `fixtures/<fixture>/head` is copied over and committed. The head
    /// overlay is DELETION-AWARE (same pattern as conservation_tests'
    /// `overlay_fixture`): every base-fixture file the head fixture does
    /// not contain is removed before the head commit, so a head tree
    /// that omits a base file really deletes it (e.g. the `moved_*`
    /// fixtures: `old.rs` is gone at head). Without this, a pure
    /// copy-over kept deleted files alive at head and the moved-file
    /// pairing could never exercise the absent-at-head rule.
    fn two_commit(fixture: &str) -> Self {
        let repo = TempRepo::with_fixture(&format!("{fixture}/base"));
        let base_sha = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
        // Idempotent re-copy: yields the base fixture's copied file set
        // for the deletion-aware overlay below.
        let base_files = copy_fixture(&repo.root, &format!("{fixture}/base"));
        let head_files = copy_fixture(&repo.root, &format!("{fixture}/head"));
        // A base file survives at head only when the head fixture tree
        // contains a file at the same repo-relative path; everything
        // else is removed before the head commit.
        for rel in &base_files {
            if !head_files.contains(rel) {
                let _ = std::fs::remove_file(repo.root.join(rel));
            }
        }
        repo.commit_all("head");
        Self { repo, base_sha }
    }

    /// Runs `structure --paths **/*.rs --base <sha1>`.
    fn scan(&self) -> Output {
        let base = self.base_sha.clone();
        self.repo
            .run_guard(&["structure", "--paths", "**/*.rs", "--base", &base])
    }

    /// Writes `delta.toml` into the repo and scans with `--delta`.
    fn scan_with_delta(&self, delta: &str) -> Output {
        std::fs::write(self.repo.root.join("delta.toml"), delta).expect("write delta");
        let base = self.base_sha.clone();
        self.repo.run_guard(&[
            "structure",
            "--paths",
            "**/*.rs",
            "--base",
            &base,
            "--delta",
            "delta.toml",
        ])
    }
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

// ---------- findings 10a–10f (round 2) ----------

#[test]
fn pub_super_to_pub_crate_is_vis_widening() {
    // 10a: distinct visibility ranks — `pub(super) -> pub(crate)` moves
    // up a tier even though both are "restricted".
    // Counterfactual: fails without the fix because the old two-tier
    // ranking treated every restricted visibility as one tier, so the
    // head index found no growth and the run was clean.
    let repo = DiffRepo::two_commit("structure/widen_super_crate");
    assert_finding(&repo.scan(), "VIS_WIDENING");
}

#[test]
fn widened_struct_field_is_vis_widening() {
    // 10b: struct fields participate, keyed `Struct::field`.
    // Counterfactual: fails without the fix because only named items
    // were indexed, so a widened field was invisible and the run was
    // clean.
    let repo = DiffRepo::two_commit("structure/widen_field");
    assert_finding(&repo.scan(), "VIS_WIDENING");
}

#[test]
fn widened_item_in_inline_module_is_vis_widening() {
    // 10b: inline modules are recursed, so an item inside
    // `mod m { .. }` matches its base counterpart.
    // Counterfactual: fails without the fix because base and head keys
    // did not carry the module path, the item looked new (never a
    // widening) and the run was clean.
    let repo = DiffRepo::two_commit("structure/widen_super_crate");
    assert_finding(&repo.scan(), "VIS_WIDENING");
}

#[test]
fn scope_excludes_new_public_file_but_in_scope_widening_remains() {
    let out_repo = DiffRepo::two_commit("structure/new_file_widen");
    let base = out_repo.base_sha.clone();
    let outside = out_repo.repo.run_guard(&[
        "structure",
        "--paths",
        "**/*.rs",
        "--scope",
        "crates/other",
        "--base",
        &base,
    ]);
    assert_clean(&outside);

    let in_repo = DiffRepo::two_commit("structure/new_file_widen");
    let base = in_repo.base_sha.clone();
    let inside = in_repo.repo.run_guard(&[
        "structure",
        "--paths",
        "**/*.rs",
        "--scope",
        "crates/fix/src",
        "--base",
        &base,
    ]);
    assert_finding(&inside, "VIS_WIDENING");

    let unscoped = DiffRepo::two_commit("structure/new_file_widen");
    assert_finding(&unscoped.scan(), "VIS_WIDENING");
}

#[test]
fn scope_union_intersects_paths_and_changed_since() {
    // Reuses the existing new_file_widen fixture; the extra public files
    // (one in a second scope dir, one sibling-prefix that must stay OUT)
    // are written in-process and committed via the test-only git helpers
    // BEFORE the guard runs — no new fixture directory.
    //
    // The bait is committed BEFORE the base revision and is NEVER touched
    // afterwards, so `--changed-since` alone cannot select it (finding
    // 10b): only the explicit `--paths` selector brings it in, and the
    // union must keep it.
    let repo = TempRepo::with_fixture("structure/new_file_widen/base");
    let in_scope = repo.root.join("crates/fix-other/src/mod.rs");
    std::fs::create_dir_all(in_scope.parent().expect("parent")).expect("mkdir fix-other");
    std::fs::write(
        &in_scope,
        "// MODRS_LOGIC bait: paths-only selection must stay selected even\n\
         // though it is unchanged at HEAD.\n\
         pub fn helper() -> u32 { 1 }\n",
    )
    .expect("write in-scope public file");
    let sibling = repo.root.join("crates/fix-other-sibling/src/lib.rs");
    std::fs::create_dir_all(sibling.parent().expect("parent")).expect("mkdir sibling");
    std::fs::write(&sibling, "pub(crate) fn hidden() -> u32 { 3 }\n")
        .expect("write sibling-prefix public file");
    repo.commit_all("base-with-bait");
    let base = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();

    // Head overlay, deletion-aware (same recipe as `DiffRepo::two_commit`).
    let base_files = copy_fixture(&repo.root, "structure/new_file_widen/base");
    let head_files = copy_fixture(&repo.root, "structure/new_file_widen/head");
    for rel in &base_files {
        if !head_files.contains(rel) {
            let _ = std::fs::remove_file(repo.root.join(rel));
        }
    }
    repo.commit_all("head");

    // The bait is unchanged since base, so changed-since alone (scoped to
    // its own directory, where it would be the ONLY candidate) selects
    // nothing at all — proving the bait is not change-set selectable.
    let changed_only = repo.run_guard(&[
        "structure",
        "--changed-since",
        &base,
        "--scope",
        "crates/fix-other/src",
        "--base",
        &base,
    ]);
    assert_clean(&changed_only);

    // Selectors: --paths covers the unchanged bait file (the union must
    // keep it even though --changed-since drops it as unchanged),
    // --changed-since covers the diff-selected extra.rs widening. Scopes
    // admit fix/src and fix-other/src; the sibling fix-other-sibling
    // prefix must be excluded by the component boundary.
    let out = repo.run_guard(&[
        "structure",
        "--paths",
        "crates/fix-other/src/mod.rs",
        "--changed-since",
        &base,
        "--scope",
        "crates/fix/src",
        "--scope",
        "crates/fix-other/src",
        "--base",
        &base,
    ]);
    let err = stderr(&out);
    assert_finding(&out, "VIS_WIDENING");
    assert!(
        err.contains("crates/fix/src/extra.rs"),
        "changed-since-selected widening missing: {err}"
    );
    assert!(
        !err.contains("crates/fix-other-sibling/"),
        "sibling-prefix file must be scoped out: {err}"
    );
    assert!(
        !err.contains("rust-guard: error"),
        "no infra errors expected: {err}"
    );
    // The paths-selected file is unchanged at HEAD, so only a run that
    // actually scans it (paths honored) would report its MODRS_LOGIC.
    assert_finding(&out, "MODRS_LOGIC");
}

#[test]
fn component_boundary_keeps_prefix_sibling_package_out() {
    // 10b component boundary: the campaign scope names the package `fix`;
    // a package whose name merely EXTENDS that prefix (`fixtwo`) is a
    // different component — its changed files must NOT be selected, and
    // its `pub(crate)` widening must not be scanned, hence a clean run.
    let repo = DiffRepo::two_commit("structure/prefix_boundary");
    let base = repo.base_sha.clone();
    let scoped_out = repo.repo.run_guard(&[
        "structure",
        "--paths",
        "**/*.rs",
        "--scope",
        "crates/fix",
        "--changed-since",
        &base,
        "--base",
        &base,
    ]);
    assert_clean(&scoped_out);

    // Sanity: the very same run scoped to the `fixtwo` package itself
    // DOES see the widening — the first run was clean because of the
    // component boundary, not because the file was ineligible.
    let scoped_in = repo.repo.run_guard(&[
        "structure",
        "--paths",
        "**/*.rs",
        "--scope",
        "crates/fixtwo",
        "--changed-since",
        &base,
        "--base",
        &base,
    ]);
    assert_finding(&scoped_in, "VIS_WIDENING");
    assert!(stderr(&scoped_in).contains("crates/fixtwo/src/lib.rs"));
}

#[test]
fn changed_since_selector_is_intersected_with_scope() {
    // Only --changed-since drives selection; the scope keeps the diff's
    // extra.rs widening while a narrow scope on the untouched other crate
    // proves the intersection actually drops it.
    let repo = DiffRepo::two_commit("structure/new_file_widen");
    let base = repo.base_sha.clone();
    let wide = repo.repo.run_guard(&[
        "structure",
        "--changed-since",
        &base,
        "--scope",
        "crates/fix/src",
        "--base",
        &base,
    ]);
    assert_finding(&wide, "VIS_WIDENING");

    let narrow = repo.repo.run_guard(&[
        "structure",
        "--changed-since",
        &base,
        "--scope",
        "crates/fix-other/src",
        "--base",
        &base,
    ]);
    assert_clean(&narrow);
}

#[test]
fn new_file_with_pub_crate_is_vis_widening() {
    // 10c: a brand-new file introducing `pub(crate)` items widens the
    // crate's surface.
    // Counterfactual: fails without the fix because new files were
    // skipped entirely by the pairwise diff and the run was clean.
    let repo = DiffRepo::two_commit("structure/new_file_widen");
    assert_finding(&repo.scan(), "VIS_WIDENING");
}

#[test]
fn new_file_with_one_allowance_per_item_is_clean() {
    // 10c + R5: delta matching is EXACT per (path, item); EVERY
    // crate-visible item of a new file needs its own consumed entry
    // (there is no path-only wildcard form — `item` is required).
    // Counterfactual (old path-only matching): ONE entry silenced the
    // whole file - two pub(crate) items passed with a single allowance
    // and nothing was ever reported unused.
    let repo = DiffRepo::two_commit("structure/new_file_widen_two");
    let delta = "[[allow_widen]]\n\
                 path = \"crates/fix/src/extra.rs\"\n\
                 item = \"alpha\"\n\
                 reason = \"new internal helper\"\n\
                 owner = \"team-a\"\n\
                 [[allow_widen]]\n\
                 path = \"crates/fix/src/extra.rs\"\n\
                 item = \"beta\"\n\
                 reason = \"new internal helper 2\"\n\
                 owner = \"team-a\"\n\
";
    assert_clean(&repo.scan_with_delta(delta));
}

#[test]
fn new_file_second_widening_without_allowance_is_reported() {
    // R5: with ONE allowance for `alpha`, the `beta` widening still
    // fires and the allowance itself is consumed (no UNUSED_ALLOWANCE).
    // Counterfactual: fails against the old behaviour because the
    // path-only allowance suppressed BOTH items - the run was clean and
    // `beta` widened silently.
    let repo = DiffRepo::two_commit("structure/new_file_widen_two");
    let delta = "[[allow_widen]]\n\
                 path = \"crates/fix/src/extra.rs\"\n\
                 item = \"alpha\"\n\
                 reason = \"new internal helper\"\n\
                 owner = \"team-a\"\n\
";
    let out = repo.scan_with_delta(delta);
    assert_finding(&out, "VIS_WIDENING");
    let err = stderr(&out);
    assert!(
        err.contains("beta"),
        "expected the uncovered item `beta`:\n{err}"
    );
    assert!(
        !err.contains("UNUSED_ALLOWANCE"),
        "the `alpha` entry WAS consumed; no leftover expected:\n{err}"
    );
}

#[test]
fn mis_keyed_item_is_vis_widening_and_unused_allowance() {
    // Finding 9: matching is EXACT on (path, item). An entry keyed to a
    // DIFFERENT item (`other` instead of `alpha`) neither suppresses the
    // widening NOR disappears silently.
    // Counterfactual: with the old empty-item wildcard this exact
    // mis-keyed entry (a non-empty `item`) already failed to match — the
    // new part is the UNUSED_ALLOWANCE assertion through the same
    // leftover path; and without the exact-match fix an item-less
    // variant would have consumed the finding instead.
    let repo = DiffRepo::two_commit("structure/new_file_widen_two");
    let delta = "[[allow_widen]]\n\
                 path = \"crates/fix/src/extra.rs\"\n\
                 item = \"other\"\n\
                 reason = \"mis-keyed item\"\n\
                 owner = \"team-a\"\n";
    let out = repo.scan_with_delta(delta);
    assert_finding(&out, "VIS_WIDENING");
    assert!(stderr(&out).contains("UNUSED_ALLOWANCE"));
}

#[test]
fn unused_allowance_is_reported() {
    // R5: an entry that matches nothing is itself a finding.
    // Counterfactual: fails without the fix because stale entries were
    // silently ignored forever.
    let repo = DiffRepo::two_commit("structure/new_file_widen_two");
    let delta = "[[allow_widen]]\n\
                 path = \"crates/fix/src/extra.rs\"\n\
                 item = \"gamma\"\n\
                 reason = \"stale entry\"\n\
                 owner = \"team-a\"\n\
";
    assert_finding(&repo.scan_with_delta(delta), "UNUSED_ALLOWANCE");
}

#[test]
fn delta_with_no_selected_files_is_unused_allowance() {
    // Finding 9: a run whose `--paths` glob selects NOTHING previously
    // returned Ok(0) BEFORE the delta was even loaded, silently ignoring
    // a non-empty delta.
    // Counterfactual: fails pre-fix because `execute` bailed out with
    // `no matching files` before `Delta::load` — the run was clean.
    let repo = TempRepo::with_fixture("structure/new_file_widen");
    let delta = "[[allow_widen]]\n\
                 path = \"crates/fix/src/extra.rs\"\n\
                 item = \"alpha\"\n\
                 reason = \"stale entry, nothing selected\"\n\
                 owner = \"team-a\"\n";
    std::fs::write(repo.root.join("delta.toml"), delta).expect("write delta");
    let out = repo.run_guard(&[
        "structure",
        "--paths",
        "src/nonexistent/**",
        "--delta",
        "delta.toml",
    ]);
    assert_finding(&out, "UNUSED_ALLOWANCE");
}

#[test]
fn delta_with_empty_item_is_malformed() {
    let repo = DiffRepo::two_commit("structure/new_file_widen");
    // Counterfactual: without validating whitespace-only item keys, this
    // otherwise valid delta reaches matching instead of returning malformed.
    // scan_with_delta supplies repo.base_sha, so this pins item validation.
    let delta = "[[allow_widen]]\npath = \"crates/fix/src/lib.rs\"\nitem = \"  \"\nreason = \"empty item\"\nowner = \"team-a\"\n";
    let out = repo.scan_with_delta(delta);
    let err = stderr(&out);
    assert!(!out.status.success(), "expected validation failure: {err}");
    assert!(
        err.contains("non-empty item"),
        "expected item validation: {err}"
    );
}

#[test]
fn delta_with_missing_item_is_malformed() {
    let repo = DiffRepo::two_commit("structure/new_file_widen");
    // `item` is a required delta field; this pins deserialize-time rejection.
    let delta = "[[allow_widen]]\npath = \"crates/fix/src/lib.rs\"\nreason = \"missing item\"\nowner = \"team-a\"\n";
    let out = repo.scan_with_delta(delta);
    let err = stderr(&out);
    assert!(!out.status.success(), "expected validation failure: {err}");
    assert!(
        err.contains("cannot parse") && err.contains("item"),
        "expected missing-item parse validation: {err}"
    );
}

#[test]
fn delta_without_base_is_unused_allowance() {
    // Finding 9: without `--base`/`--changed-since` the diff (the only
    // consumer of delta entries) never runs; a non-empty delta was
    // silently ignored.
    // Counterfactual: fails pre-fix because the no-base path never
    // reported leftovers — the run was clean.
    let repo = TempRepo::with_fixture("structure/new_file_widen");
    let delta = "[[allow_widen]]\n\
                 path = \"crates/fix/src/extra.rs\"\n\
                 item = \"alpha\"\n\
                 reason = \"stale entry, no base\"\n\
                 owner = \"team-a\"\n";
    std::fs::write(repo.root.join("delta.toml"), delta).expect("write delta");
    let out = repo.run_guard(&["structure", "--paths", "**/*.rs", "--delta", "delta.toml"]);
    assert_finding(&out, "UNUSED_ALLOWANCE");
}
#[test]
fn allow_added_with_wrong_item_still_reports() {
    // R5: an entry whose `item` does not equal the normalised attribute
    // text neither suppresses the finding NOR disappears silently.
    // Counterfactual: fails without the fix because matching was by
    // path only - the mis-keyed entry suppressed ALLOW_ADDED and no
    // UNUSED_ALLOWANCE ever surfaced.
    let repo = DiffRepo::two_commit("structure/allow_added");
    let delta = "[[allow]]\n\
                 path = \"crates/fix/src/lib.rs\"\n\
                 item = \"helper\"\n\
                 reason = \"mis-keyed item\"\n\
                 owner = \"team-a\"\n\
";
    let out = repo.scan_with_delta(delta);
    assert_finding(&out, "ALLOW_ADDED");
    assert!(stderr(&out).contains("UNUSED_ALLOWANCE"));
}

#[test]
fn qualified_owned_impl_outside_module_is_reported() {
    // 10d: the governed self-type name is the LAST segment of a
    // qualified path (`impl crate::repo::instance::RepoInstance`).
    // Counterfactual: fails without the fix because only a single-segment
    // self type matched, so the qualified impl escaped the rule and the
    // run was clean.
    let repo = TempRepo::with_fixture("structure/owned_impl_qualified");
    assert_finding(&scan(&repo), "OWNED_IMPL");
}

#[test]
fn local_use_inside_impl_method_is_reported() {
    // 10e: the local-`use` traversal covers the whole file (impl method
    // bodies, closures, loops, conditionals).
    // Counterfactual: fails without the fix because only top-level fn
    // bodies were walked, so a `use` inside an impl method was missed
    // and the run was clean.
    let repo = TempRepo::with_fixture("structure/local_use_in_impl");
    assert_finding(&scan(&repo), "LOCAL_USE");
}

#[test]
fn new_allow_is_reported_and_accepted_by_delta() {
    // A file gains `#[allow(dead_code)]` between base and head:
    // ALLOW_ADDED; with a matching `[[allow]]` delta entry the same run
    // is clean.
    // Counterfactual: fails without the ALLOW_ADDED rule because the
    // pairwise diff only compared visibility — a newly added `#[allow]`
    // attribute slipped in silently and the run was clean.
    let repo = DiffRepo::two_commit("structure/allow_added");
    assert_finding(&repo.scan(), "ALLOW_ADDED");

    let delta = "[[allow]]\n\
                 path = \"crates/fix/src/lib.rs\"\n\
                 item = \"#[allow(dead_code)]\"\n\
                 reason = \"intentionally dead helper\"\n\
                 owner = \"team-a\"\n";
    assert_clean(&repo.scan_with_delta(delta));
}

#[test]
fn logic_in_inline_module_of_mod_rs_is_reported() {
    // 10f: check_mod_rs recurses into inline modules.
    // Counterfactual: fails without the fix because only the top-level
    // items of a mod.rs were checked, so `mod inner { pub fn f() {} }`
    // hid logic and the run was clean.
    let repo = TempRepo::with_fixture("structure/modrs_inline");
    assert_finding(&scan(&repo), "MODRS_LOGIC");
}

// ---------- round 3 (R5/R7/R9) ----------

#[test]
fn moved_file_that_adds_allow_is_allow_added() {
    // R7d: a file MOVED from old.rs to new.rs (identical bodies) is
    // paired with its base counterpart, so a newly added `#[allow]` is
    // still an ALLOW_ADDED finding.
    // Counterfactual: if pairing fails, the new-file branch reports BOTH
    // VIS_WIDENING and ALLOW_ADDED. The moved-file INFO line proves pairing;
    // asserting no VIS_WIDENING pins that behavior.
    let repo = DiffRepo::two_commit("structure/moved_adds_allow");
    let out = repo.scan();
    assert_finding(&out, "ALLOW_ADDED");
    assert!(
        !stderr(&out).contains("VIS_WIDENING"),
        "moved file must not be checked as a new file: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("paired with base `crates/fix/src/old.rs` as a moved file"),
        "expected the moved-file pairing INFO line:\n{}",
        stderr(&out)
    );
}

#[test]
fn moved_file_that_widens_is_vis_widening() {
    // R7d: the moved file widened `hidden` to `pub(crate)`. Even with
    // attribute-insensitive fingerprints the widened item's fingerprint
    // differs (visibility IS hashed), so the signature multiset can't
    // match anyway; pairing additionally requires head vis <= base vis
    // per item. The file falls back to the new-file check, which still
    // reports VIS_WIDENING.
    // Counterfactual: fails without moved-file pairing at all because
    // the file was absent from base - pre-R7d code reported nothing for
    // it (new files were skipped rather than item-checked).
    let repo = DiffRepo::two_commit("structure/moved_widen");
    assert_finding(&repo.scan(), "VIS_WIDENING");
}

#[test]
fn moved_file_with_unchanged_content_is_clean() {
    // Negative control (finding 11): a TRUE move (old.rs deleted at head
    // via the deletion-aware harness) whose content is byte-identical
    // must pair as moved and produce a fully CLEAN run (exit 0, "0
    // finding") — not merely an absence of VIS_WIDENING, which an infra
    // failure would also satisfy vacuously.
    // Counterfactual: fails pre-fix on BOTH counts — the copy-over
    // harness kept old.rs alive at head (no absent-at-head candidate),
    // and the old assertion only checked for missing VIS_WIDENING.
    let repo = DiffRepo::two_commit("structure/moved_plain");
    assert_clean(&repo.scan());
}

#[test]
fn copy_is_not_moved_is_vis_widening() {
    // Finding 8: a COPY of a base file (lib.rs kept at head, extra.rs
    // added with IDENTICAL content) must not pair as a move — a base
    // file still present at head is a copy source, never a move source.
    // Counterfactual: fails pre-fix because `find_moved` scanned ALL
    // base .rs files without requiring the paired base file to be absent
    // at head, so extra.rs paired with lib.rs (signature multiset and
    // vis equal), no new-file check ran and the run was clean instead of
    // reporting VIS_WIDENING for extra.rs's `pub(crate) fn hidden`.
    let repo = DiffRepo::two_commit("structure/copy_of_base_file");
    let out = repo.scan();
    assert_finding(&out, "VIS_WIDENING");
    let err = stderr(&out);
    assert!(
        err.contains("extra.rs"),
        "expected the finding on the COPY extra.rs:\n{err}"
    );
    assert!(
        !err.contains("paired with base"),
        "a copy must not pair as a moved file:\n{err}"
    );
}

#[test]
fn selected_missing_worktree_file_is_skipped() {
    // True counterfactual: previously a worktree-only removal selected no
    // git-index path, so this test asserted clean without reaching Missing.
    // Keeping the path indexed now exercises the read_checked NotFound arm.
    let repo = TempRepo::with_fixture("structure/new_file_widen");
    std::fs::remove_file(repo.root.join("crates/fix/src/lib.rs"))
        .expect("remove tracked working-tree file");
    let out = repo.run_guard(&["structure", "--paths", "crates/fix/src/lib.rs"]);
    assert_clean(&out);
}

#[test]
fn selected_invalid_utf8_worktree_file_is_unreadable() {
    // `ls-files` still selects this indexed path; [ff fe] is invalid UTF-8.
    // InvalidData maps to Io, not Missing; removing that error report makes
    // this assertion fail, while the companion test pins the NotFound skip.
    let repo = TempRepo::with_fixture("structure/new_file_widen");
    std::fs::write(repo.root.join("crates/fix/src/lib.rs"), [0xff, 0xfe])
        .expect("write invalid UTF-8");
    let out = repo.run_guard(&["structure", "--paths", "crates/fix/src/lib.rs"]);
    assert_finding(&out, "UNREADABLE_FILE");
}

#[test]
fn trait_impl_outside_owner_is_owned_impl() {
    // R7a: `impl Trait for RepoInstance` is governed like an inherent
    // impl; the self type's LAST segment decides. `src/other.rs` maps
    // to logical module `crate::other`, which is NOT under the owner
    // module `crate::repo::instance::repo_instance`, so OWNED_IMPL
    // fires.
    // Counterfactual (both halves): fails without the trait-impl fix
    // because trait impls were skipped entirely (`imp.trait_.is_some()`);
    // AND it failed again after that fix while the FIXTURE config was
    // wrong — root-level `type`/`module` keys (no `[[owned_impl]]`
    // table) were loaded by serde into an empty vector (unknown keys
    // ignored), so no type was governed and the run was clean.
    let repo = TempRepo::with_fixture("structure/owned_impl_trait");
    assert_finding(&scan(&repo), "OWNED_IMPL");
}

#[test]
fn widened_tuple_field_is_vis_widening() {
    // R7b: tuple-struct fields are indexed by position (`S::0`).
    // Counterfactual: fails without the fix because tuple fields have
    // no `ident` and were skipped, so widening `S.0` was silent.
    let repo = DiffRepo::two_commit("structure/widen_tuple_field");
    let out = repo.scan();
    assert_finding(&out, "VIS_WIDENING");
    assert!(
        stderr(&out).contains("S::0"),
        "expected key S::0:\n{}",
        stderr(&out)
    );
}

#[test]
fn same_named_impls_in_two_inline_modules_do_not_collapse() {
    // R7c: impl-member keys carry the inline-module prefix, so widening
    // `a::S::f` does not shadow `b::S::f` - exactly one finding.
    // Counterfactual: fails without the fix because both impls keyed to
    // `S::f`; the head index overwrote one entry, and (with only one of
    // the two widened) the widened module's finding was lost.
    let repo = DiffRepo::two_commit("structure/impl_two_inline_modules");
    let out = repo.scan();
    assert_finding(&out, "VIS_WIDENING");
    let err = stderr(&out);
    assert!(err.contains("a::S::f"), "expected a::S::f:\n{err}");
    assert!(!err.contains("b::S::f"), "b must be untouched:\n{err}");
}

#[test]
fn new_file_allow_is_allow_added() {
    // R7d: brand-new files also run the ALLOW_ADDED check (against an
    // empty base), not only the visibility check.
    // Counterfactual: fails without the fix because new files were only
    // checked for `pub(crate)`+ items; a private fn hidden behind a
    // fresh `#[allow(dead_code)]` entered silently.
    let repo = DiffRepo::two_commit("structure/new_file_allow");
    assert_finding(&repo.scan(), "ALLOW_ADDED");
}

// ---------- split files are compared with the vacated base files ----------

#[test]
fn split_files_carrying_base_items_are_clean() {
    // `old.rs` is split into `part_a.rs`/`part_b.rs`; every `pub(crate)`
    // item, `#[allow]`, inline test module and function-local `use` it
    // carries existed in the deleted base file, so none is a new finding.
    // Counterfactual: without the vacated-base comparison the new files
    // are judged against an empty base and report VIS_WIDENING,
    // ALLOW_ADDED, INLINE_TEST_MOD and LOCAL_USE.
    let repo = DiffRepo::two_commit("structure/split_clean");
    assert_clean(&repo.scan());
}

#[test]
fn split_file_that_widens_an_item_is_vis_widening() {
    // `a` was `pub(crate)` in the vacated base file and is `pub` now.
    // Counterfactual: if any base item with the same key excused the head
    // item regardless of rank, this widening would pass silently.
    let repo = DiffRepo::two_commit("structure/split_widened");
    let out = repo.scan();
    assert_finding(&out, "VIS_WIDENING");
    assert!(
        stderr(&out).contains("new file introduces `a` at pub visibility"),
        "expected the widening of `a`:\n{}",
        stderr(&out)
    );
}

#[test]
fn split_file_with_a_new_allow_is_allow_added() {
    // `dead_code` was in the vacated base file; `unused` is new.
    // Counterfactual: if the whole vacated set excused every attribute the
    // new `#[allow(unused)]` would pass.
    let repo = DiffRepo::two_commit("structure/split_new_allow");
    let out = repo.scan();
    assert_finding(&out, "ALLOW_ADDED");
    let err = stderr(&out);
    assert!(err.contains("unused"), "expected the new allow:\n{err}");
    assert!(
        !err.contains("dead_code"),
        "the base allow must stay excused:\n{err}"
    );
}

#[test]
fn copy_of_a_base_file_is_not_a_vacated_source() {
    // `old.rs` still exists at head, so it is a copy source and nothing was
    // vacated: the inline test module of the new `part.rs` stays a finding.
    // Counterfactual: indexing every base file (not only deleted ones)
    // would excuse it.
    let repo = DiffRepo::two_commit("structure/split_copy_not_vacated");
    let out = repo.scan();
    assert!(
        stderr(&out)
            .lines()
            .any(|l| l.contains("part.rs") && l.contains("INLINE_TEST_MOD")),
        "expected INLINE_TEST_MOD for part.rs:\n{}",
        stderr(&out)
    );
}
