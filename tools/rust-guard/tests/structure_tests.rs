//! Self-tests for the `structure` gate (CI-only): one fixture repo per
//! rule, the binary invoked with `--paths **/*.rs`.

mod harness;

use crate::harness::{assert_clean, assert_finding, copy_fixture, TempRepo};
use std::process::Output;

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
    /// `fixtures/<fixture>/head` is copied over and committed.
    fn two_commit(fixture: &str) -> Self {
        let repo = TempRepo::with_fixture(&format!("{fixture}/base"));
        let base_sha = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
        copy_fixture(&repo.root, &format!("{fixture}/head"));
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
fn new_file_with_pub_crate_is_vis_widening() {
    // 10c: a brand-new file introducing `pub(crate)` items widens the
    // crate's surface.
    // Counterfactual: fails without the fix because new files were
    // skipped entirely by the pairwise diff and the run was clean.
    let repo = DiffRepo::two_commit("structure/new_file_widen");
    assert_finding(&repo.scan(), "VIS_WIDENING");
}

#[test]
fn new_file_widening_is_accepted_by_delta() {
    // 10c: the same repo with `[[allow_widen]]` covering the path is
    // clean.
    // Counterfactual: fails without the fix because there was no
    // new-file check at all — the delta entry matched nothing and, more
    // importantly, the ungoverned widening slipped through silently in
    // the pre-fix guard.
    let repo = DiffRepo::two_commit("structure/new_file_widen");
    let delta = "[[allow_widen]]\n\
                 path = \"crates/fix/src/extra.rs\"\n\
                 item = \"added\"\n\
                 reason = \"new internal helper\"\n\
                 owner = \"team-a\"\n";
    assert_clean(&repo.scan_with_delta(delta));
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
                 item = \"helper\"\n\
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
