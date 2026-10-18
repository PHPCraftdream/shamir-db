//! Self-tests for the `conservation` gate (CI-only): each fixture is a
//! two-commit throwaway repo (base/head trees), compared via `--base
//! <sha1> --head HEAD` with an optional delta file.

mod harness;

use crate::harness::{assert_clean, assert_finding, TempRepo};

const LIB: &str = "crates/fix/src/lib.rs";

fn compare(repo: &TempRepo) -> std::process::Output {
    let base = repo.base_sha.clone();
    repo.run_guard(&["conservation", "--base", &base, "--head", "HEAD"])
}

fn compare_with_delta(repo: &TempRepo, delta: &str) -> std::process::Output {
    repo.write("delta.toml", delta);
    let base = repo.base_sha.clone();
    repo.run_guard(&[
        "conservation",
        "--base",
        &base,
        "--head",
        "HEAD",
        "--delta",
        "delta.toml",
    ])
}

#[test]
fn deleted_test_is_never_declarable() {
    let repo = TempRepo::two_commit("conservation/test_lost");
    assert_finding(&compare(&repo), "TEST_LOST");
}

#[test]
fn newly_ignored_test_is_reported() {
    let repo = TempRepo::two_commit("conservation/test_ignored");
    assert_finding(&compare(&repo), "TEST_IGNORED");
}

#[test]
fn declared_rename_is_accepted() {
    let repo = TempRepo::two_commit("conservation/renamed_ok");
    let delta = format!(
        "base = \"{}\"\n\n\
         [[renamed]]\n\
         path = \"{LIB}\"\n\
         item = \"helper\"\n\
         to_item = \"helper2\"\n\
         reason = \"clearer name\"\n\
         owner = \"@rust-guard\"\n",
        repo.base_sha
    );
    assert_clean(&compare_with_delta(&repo, &delta));
}

#[test]
fn undeclared_body_change_is_reported() {
    let repo = TempRepo::two_commit("conservation/body_changed");
    assert_finding(&compare(&repo), "BODY_CHANGED");
}

#[test]
fn unmatched_delta_entry_is_reported() {
    let repo = TempRepo::two_commit("conservation/unused_allowance");
    let delta = format!(
        "base = \"{}\"\n\n\
         [[changed]]\n\
         path = \"{LIB}\"\n\
         item = \"no_such_item\"\n\
         reason = \"typo in the delta\"\n\
         owner = \"@rust-guard\"\n",
        repo.base_sha
    );
    assert_finding(&compare_with_delta(&repo, &delta), "UNUSED_ALLOWANCE");
}

#[test]
fn wrong_base_sha_is_reported() {
    let repo = TempRepo::two_commit("conservation/base_mismatch");
    let delta = "base = \"deadbeefdead\"\n\n\
                 [[changed]]\n\
                 path = \"crates/fix/src/lib.rs\"\n\
                 item = \"helper\"\n\
                 reason = \"stale base pin\"\n\
                 owner = \"@rust-guard\"\n";
    assert_finding(&compare_with_delta(&repo, delta), "BASE_MISMATCH");
}

#[test]
fn duplicate_head_candidates_are_unpairable() {
    // One base item, two head candidates with the same kind/name/
    // enclosing and signature — matching.rs cannot choose a pairing.
    let repo = TempRepo::two_commit("conservation/unpairable_duplicate");
    assert_finding(&compare(&repo), "UNPAIRABLE_DUPLICATE");
}

#[test]
fn missing_base_rev_blocks_the_comparison() {
    let repo = TempRepo::two_commit("conservation/test_lost");
    assert_finding(
        &repo.run_guard(&["conservation", "--base", "does-not-exist", "--head", "HEAD"]),
        "BLOCKED_COMPARISON",
    );
}
