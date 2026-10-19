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
fn new_file_with_one_allowance_per_item_is_clean() {
    // 10c + R5: delta matching is per (path, item); EVERY crate-visible
    // item of a new file needs its own consumed entry (an empty `item`
    // acts as a documented wildcard).
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
    // Counterfactual: fails without attribute-insensitive fingerprints
    // because syn prints the added `#[allow(dead_code)]` into the item's
    // token stream, so new.rs's fingerprint multiset matches no base
    // file; the moved-file pairing never fires and the new-file check
    // reports VIS_WIDENING for the pub fn instead of ALLOW_ADDED.
    let repo = DiffRepo::two_commit("structure/moved_adds_allow");
    assert_finding(&repo.scan(), "ALLOW_ADDED");
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
    // Negative control: a move whose only delta is an added `#[allow]`
    // must still pair (fingerprints are attribute-insensitive) and so
    // must produce zero VIS_WIDENING (the ALLOW_ADDED finding is
    // covered by the dedicated test above).
    let repo = DiffRepo::two_commit("structure/moved_adds_allow");
    let out = repo.scan();
    assert!(
        !stderr(&out).contains("VIS_WIDENING"),
        "pure move must not widen:\n{}",
        stderr(&out)
    );
}

#[test]
fn trait_impl_outside_owner_is_owned_impl() {
    // R7a: `impl Trait for RepoInstance` is governed like an inherent
    // impl; the self type's LAST segment decides.
    // Counterfactual: fails without the fix because trait impls were
    // skipped entirely (`imp.trait_.is_some()`), so the trait impl
    // outside the owner module escaped the rule silently.
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
