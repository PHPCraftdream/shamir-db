//! Self-tests for the `conservation` gate (CI-only): each fixture is a
//! two-commit throwaway repo (base/head trees), compared via `--base
//! <sha1> --head HEAD` with an optional delta file.

mod harness;

use crate::harness::{assert_clean, assert_finding, copy_fixture, TempRepo};
use std::path::Path;
use std::process::Output;

/// Copies `fixtures/<fixture>` over the repo AND DELETES every file of
/// `previous` (the base fixture's copied set) that the new fixture does
/// not contain — the head overlay of a two-commit fixture may REMOVE
/// files (a deleted test target), and a pure copy-over would silently
/// keep them in the head commit.
fn overlay_fixture(dst: &Path, fixture: &str, previous: &[String]) {
    let copied = copy_fixture(dst, fixture);
    for rel in previous {
        if !copied.contains(rel) {
            let _ = std::fs::remove_file(dst.join(rel));
        }
    }
}

/// A two-commit repo plus the first commit's SHA (`--base` argument).
struct ConsRepo {
    repo: TempRepo,
    /// SHA of the first commit (the fixture's `base/` tree).
    base_sha: String,
}

impl ConsRepo {
    /// Two-commit repo: `fixtures/<fixture>/base` is committed first,
    /// then `fixtures/<fixture>/head` is copied over and committed (the
    /// commit is `--allow-empty`, so identical trees still work).
    ///
    /// The head overlay DELETES every base-fixture file the head fixture
    /// does not contain — a head revision may remove a file (a deleted
    /// test target), and a pure copy-over would silently keep it in the
    /// head commit, hiding exactly the deletions the gate must catch.
    /// Fixtures whose head contains every base file are unaffected.
    fn two_commit(fixture: &str) -> Self {
        let repo = TempRepo::with_fixture(&format!("{fixture}/base"));
        let base_sha = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
        // Idempotent re-copy: yields the base fixture's copied file set
        // for the deletion-aware overlay below.
        let base_files = copy_fixture(&repo.root, &format!("{fixture}/base"));
        overlay_fixture(&repo.root, &format!("{fixture}/head"), &base_files);
        repo.commit_all("head");
        Self { repo, base_sha }
    }

    /// Writes `contents` to `rel` inside the repo (delta files).
    fn write(&self, rel: &str, contents: &str) {
        let path = self.repo.root.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("create dir");
        }
        std::fs::write(path, contents).expect("write file");
    }

    /// Runs `conservation --base <sha1> --head HEAD`.
    fn compare(&self) -> Output {
        let base = self.base_sha.clone();
        self.repo
            .run_guard(&["conservation", "--base", &base, "--head", "HEAD"])
    }

    /// Writes `delta` to the repo and runs the comparison with `--delta`.
    fn compare_with_delta(&self, delta: &str) -> Output {
        self.write("delta.toml", delta);
        let base = self.base_sha.clone();
        self.repo.run_guard(&[
            "conservation",
            "--base",
            &base,
            "--head",
            "HEAD",
            "--delta",
            "delta.toml",
        ])
    }
    /// Runs the comparison with `--delta` pointing at an existing file
    /// in the repo (no write).
    fn compare_with_delta_file(&self, rel: &str) -> Output {
        let base = self.base_sha.clone();
        self.repo.run_guard(&[
            "conservation",
            "--base",
            &base,
            "--head",
            "HEAD",
            "--delta",
            rel,
        ])
    }

    /// Runs the comparison with `--suggest-delta <out_rel>`; the
    /// suggestion file (if any) is written relative to the repo root
    /// (the guard's cwd) and read back via `read`.
    fn compare_with_suggest(&self, out_rel: &str) -> Output {
        let base = self.base_sha.clone();
        self.repo.run_guard(&[
            "conservation",
            "--base",
            &base,
            "--head",
            "HEAD",
            "--suggest-delta",
            out_rel,
        ])
    }

    /// Reads a file inside the repo (suggestion files).
    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.repo.root.join(rel)).expect("read file")
    }

    /// Whether the file exists inside the repo (suggestion files).
    fn read_ok(&self, rel: &str) -> bool {
        self.repo.root.join(rel).exists()
    }
}

/// stderr as text — the guard prints its findings there.
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn contains_sub(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// P3 parity: `--suggest-delta` must not change the gate's output apart
/// from the note line(s) — same exit code, identical stdout, and a
/// stderr that is the plain stderr plus note line(s) only. All three
/// comparisons are byte-exact (no lossy UTF-8, no line-ending
/// normalisation); the note line(s) are stripped as raw bytes.
fn assert_flagless_output_unchanged(flagged: &Output, plain: &Output, note_marker: &str) {
    assert_eq!(
        flagged.status.code(),
        plain.status.code(),
        "exit code changed by --suggest-delta"
    );
    assert_eq!(
        flagged.stdout, plain.stdout,
        "stdout changed by --suggest-delta"
    );
    // Remove exactly the note line(s) from the flagged stderr, as raw
    // bytes (no lossy UTF-8, no line-ending normalisation).
    let marker = note_marker.as_bytes();
    let stripped: Vec<u8> = flagged
        .stderr
        .split_inclusive(|&b| b == b'\n')
        .filter(|line| !contains_sub(line, marker))
        .flatten()
        .copied()
        .collect();
    assert_eq!(
        stripped, plain.stderr,
        "stderr changed by --suggest-delta beyond the note line(s)"
    );
}

const LIB: &str = "crates/fix/src/lib.rs";

#[test]
fn deleted_test_is_never_declarable() {
    let repo = ConsRepo::two_commit("conservation/test_lost");
    assert_finding(&repo.compare(), "TEST_LOST");
}

#[test]
fn newly_ignored_test_is_reported() {
    let repo = ConsRepo::two_commit("conservation/test_ignored");
    assert_finding(&repo.compare(), "TEST_IGNORED");
}

#[test]
fn declared_rename_is_accepted() {
    // Counterfactual: before the rename comparison normalised the item's
    // own name out of the signature tokens, `fn helper` vs `fn helper2`
    // could never pair (signatures differ) — this fixture would report
    // ITEM_LOST + ITEM_ADDED instead of passing.
    let repo = ConsRepo::two_commit("conservation/renamed_ok");
    let delta = format!(
        "base = \"{}\"\n\n\
         [[renamed]]\n\
         path = \"{LIB}\"\n\
         item = \"fix::lib::helper\"\n\
         to_item = \"fix::lib::helper2\"\n\
         reason = \"clearer name\"\n\
         owner = \"@rust-guard\"\n",
        repo.base_sha
    );
    assert_clean(&repo.compare_with_delta(&delta));
}

#[test]
fn undeclared_rename_is_a_loss_and_an_addition() {
    // The same rename without a [[renamed]] declaration must NOT pass:
    // the items fall apart into a loss + an addition.
    let repo = ConsRepo::two_commit("conservation/renamed_ok");
    let out = repo.compare();
    let err = stderr(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(
        err.contains("ITEM_LOST") && err.contains("ITEM_ADDED"),
        "expected ITEM_LOST + ITEM_ADDED:\n{err}"
    );
}

#[test]
fn declared_test_rename_is_accepted() {
    // Counterfactual: fails without the rename pairing's TEST handling
    // because pass 3 only pairs when kind/enclosing/fingerprint and the
    // name-normalised signature are equal — before that, a renamed
    // `#[test]` fn fell apart into a loss + an addition; and as a TEST it
    // can never be excused by `[[removed]]`/`[[added]]`, so the run could
    // never come back clean.
    let repo = ConsRepo::two_commit("conservation/renamed_test");
    let delta = format!(
        "base = \"{}\"\n\n\
         [[renamed]]\n\
         path = \"{LIB}\"\n\
         item = \"fix::lib::helper\"\n\
         to_item = \"fix::lib::helper2\"\n\
         reason = \"clearer name\"\n\
         owner = \"@rust-guard\"\n",
        repo.base_sha
    );
    assert_clean(&repo.compare_with_delta(&delta));
}

#[test]
fn undeclared_test_rename_is_a_lost_test() {
    // With no [[renamed]] entry the rename pair is rejected in pass 3.
    // Counterfactual (assertion vs ITEM_LOST + ITEM_ADDED): an undeclared
    // renamed TEST yields TEST_LOST only — the head side of the rejected
    // pair is itself a test, and new test items are never additions, so
    // reporting ITEM_ADDED here would be wrong.
    let repo = ConsRepo::two_commit("conservation/renamed_test");
    let out = repo.compare();
    let err = stderr(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(err.contains("TEST_LOST"), "expected TEST_LOST:\n{err}");
    assert!(
        !err.contains("ITEM_ADDED"),
        "the renamed test's head side must not be an addition:\n{err}"
    );
}

#[test]
fn test_cfg_change_is_reported() {
    // INHERITED module cfg (round 4): the whole inline test module `m`
    // carries a `#[cfg]` that differs between the revisions; the fn has
    // no cfg of its own, so its composed (inherited) cfg text drifts
    // v1 -> v2.
    // Counterfactual: fails without the cfg comparison in
    // report_test_change because an identity pair was compared on kind,
    // ignore status and fingerprint only — a test whose inherited
    // module cfg changed between revisions paired silently and the run
    // was clean.
    let repo = ConsRepo::two_commit("conservation/test_cfg_changed");
    assert_finding(&repo.compare(), "TEST_ATTR_CHANGED");
}

#[test]
fn assoc_type_value_change_is_signature_changed() {
    // The impl-type member's VALUE (`type A = u8;` -> `type A = u16;`)
    // is extracted with the whole item as its signature tokens and the
    // type expression as its body — so the drift is reported as
    // SIGNATURE_CHANGED (the signature text `type A = u8 ;` differs);
    // the body-only code would be wrong here because the whole item
    // token stream IS the signature for type members.
    // Counterfactual: fails if ImplItem::Type members were dropped by
    // `impl_member` (item.rs) — the member never entered the inventory,
    // the pair could not drift and the run was clean.
    let repo = ConsRepo::two_commit("conservation/assoc_type_changed");
    let out = repo.compare();
    let err = stderr(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(
        err.contains("SIGNATURE_CHANGED"),
        "expected SIGNATURE_CHANGED:\n{err}"
    );
}

#[test]
fn undeclared_body_change_is_reported() {
    let repo = ConsRepo::two_commit("conservation/body_changed");
    assert_finding(&repo.compare(), "BODY_CHANGED");
}

#[test]
fn signature_change_with_identical_body_is_reported() {
    // Counterfactual: exact-identity pairs were compared on fingerprint
    // and test attrs only; a parameter/return type change with the same
    // body (same fingerprint) passed silently. The signature comparison
    // makes it SIGNATURE_CHANGED.
    let repo = ConsRepo::two_commit("conservation/signature_changed");
    assert_finding(&repo.compare(), "SIGNATURE_CHANGED");
}

#[test]
fn unmatched_delta_entry_is_reported() {
    // The base and head trees are IDENTICAL: this exercises the
    // `--allow-empty` head commit (finding 9) — before it, `git commit`
    // failed here before the guard ever ran.
    let repo = ConsRepo::two_commit("conservation/unused_allowance");
    let delta = format!(
        "base = \"{}\"\n\n\
         [[changed]]\n\
         path = \"{LIB}\"\n\
         item = \"fix::lib::no_such_item\"\n\
         reason = \"typo in the delta\"\n\
         owner = \"@rust-guard\"\n",
        repo.base_sha
    );
    assert_finding(&repo.compare_with_delta(&delta), "UNUSED_ALLOWANCE");
}

#[test]
fn wrong_base_sha_is_reported() {
    // The head tree carries a real (comment-only) diff, so this fixture
    // exercises the non-empty-commit path; `unused_allowance` covers the
    // `--allow-empty` path.
    let repo = ConsRepo::two_commit("conservation/base_mismatch");
    let delta = "base = \"deadbeefdead\"\n\n\
                 [[changed]]\n\
                 path = \"crates/fix/src/lib.rs\"\n\
                 item = \"fix::lib::helper\"\n\
                 reason = \"stale base pin\"\n\
                 owner = \"@rust-guard\"\n";
    assert_finding(&repo.compare_with_delta(delta), "BASE_MISMATCH");
}

#[test]
fn duplicate_head_candidates_are_unpairable() {
    // One base item, two head candidates with the same kind/name/
    // enclosing and signature — matching.rs cannot choose a pairing.
    let repo = ConsRepo::two_commit("conservation/unpairable_duplicate");
    assert_finding(&repo.compare(), "UNPAIRABLE_DUPLICATE");
}

#[test]
fn ambiguous_moves_list_their_candidates() {
    // `make` exists in a.rs and b.rs at base and in c.rs and d.rs at head
    // with different bodies, so no heuristic can pair them.
    // Counterfactual: the finding used to name only the key, leaving the
    // author to hunt for the base items and head candidates by hand.
    let repo = ConsRepo::two_commit("conservation/declared_moves");
    let out = repo.compare();
    assert_finding(&out, "UNPAIRABLE_DUPLICATE");
    let err = stderr(&out);
    assert!(
        err.contains("crates/fix/src/a.rs:1") && err.contains("crates/fix/src/d.rs:1"),
        "expected the base items and head candidates:\n{err}"
    );
}

#[test]
fn declared_moves_settle_an_ambiguous_key() {
    // The same fixture with an explicit [[moved]] entry per base item
    // (a -> c, b -> d) plus the [[changed]] entries for the new bodies.
    // Counterfactual: without the declared pass the ambiguous key drops
    // the head items and the entries could never be consumed.
    let repo = ConsRepo::two_commit("conservation/declared_moves");
    let delta = format!(
        "base = \"{}\"\n\n\
         [[moved]]\n\
         path = \"crates/fix/src/a.rs\"\n\
         item = \"fix::lib::a::make\"\n\
         to_path = \"crates/fix/src/c.rs\"\n\
         to_item = \"fix::lib::c::make\"\n\
         reason = \"relocated\"\n\
         owner = \"@rust-guard\"\n\n\
         [[moved]]\n\
         path = \"crates/fix/src/b.rs\"\n\
         item = \"fix::lib::b::make\"\n\
         to_path = \"crates/fix/src/d.rs\"\n\
         to_item = \"fix::lib::d::make\"\n\
         reason = \"relocated\"\n\
         owner = \"@rust-guard\"\n\n\
         [[changed]]\n\
         path = \"crates/fix/src/a.rs\"\n\
         item = \"fix::lib::a::make\"\n\
         reason = \"new body\"\n\
         owner = \"@rust-guard\"\n\n\
         [[changed]]\n\
         path = \"crates/fix/src/b.rs\"\n\
         item = \"fix::lib::b::make\"\n\
         reason = \"new body\"\n\
         owner = \"@rust-guard\"\n",
        repo.base_sha
    );
    assert_clean(&repo.compare_with_delta(&delta));
}

#[test]
fn undeclared_cross_package_move_is_a_loss() {
    // `carried` and its test leave package `fix` for package `dest`.
    // Counterfactual: moving across packages needs the same explicit
    // declaration as a move inside one.
    let repo = ConsRepo::two_commit("conservation/cross_package_move");
    let out = repo.compare();
    assert_finding(&out, "ITEM_LOST");
    assert_finding(&out, "TEST_LOST");
}

#[test]
fn declared_cross_package_move_is_clean() {
    // Counterfactual: the declared pass keyed items by package, so a
    // `[[moved]]` entry naming a head item in another package matched
    // nothing (UNUSED_ALLOWANCE + ITEM_LOST + ITEM_ADDED + TEST_LOST).
    let repo = ConsRepo::two_commit("conservation/cross_package_move");
    let delta = format!(
        "base = \"{}\"\n\n\
         [[moved]]\n\
         path = \"crates/fix/src/lib.rs\"\n\
         item = \"fix::lib::carried\"\n\
         to_path = \"crates/dest/src/lib.rs\"\n\
         to_item = \"dest::lib::carried\"\n\
         reason = \"relocated\"\n\
         owner = \"@rust-guard\"\n\n\
         [[moved]]\n\
         path = \"crates/fix/src/lib.rs\"\n\
         item = \"fix::lib::carried_test\"\n\
         to_path = \"crates/dest/src/lib.rs\"\n\
         to_item = \"dest::lib::carried_test\"\n\
         reason = \"relocated\"\n\
         owner = \"@rust-guard\"\n",
        repo.base_sha
    );
    assert_clean(&repo.compare_with_delta(&delta));
}

/// `[[moved]]` for `take` (a.rs -> c.rs), whose signature changed.
fn moved_take_entry(base_sha: &str) -> String {
    format!(
        "base = \"{base_sha}\"\n\n\
         [[moved]]\n\
         path = \"crates/fix/src/a.rs\"\n\
         item = \"fix::lib::a::take\"\n\
         to_path = \"crates/fix/src/c.rs\"\n\
         to_item = \"fix::lib::c::take\"\n\
         reason = \"relocated\"\n\
         owner = \"@rust-guard\"\n"
    )
}

#[test]
fn declared_move_with_a_changed_signature_needs_a_changed_entry() {
    // `take(u32) -> u32` moves to c.rs as `take(u64) -> u64`; the [[moved]]
    // entry pairs the items although the signatures differ, and the drift is
    // then a SIGNATURE_CHANGED finding.
    // Counterfactual: without the declared pass the pair never forms (pass 2
    // needs equal signatures) and the item is just an ITEM_LOST + ITEM_ADDED.
    let repo = ConsRepo::two_commit("conservation/declared_moves_sig");
    let out = repo.compare_with_delta(&moved_take_entry(&repo.base_sha));
    assert_finding(&out, "SIGNATURE_CHANGED");
    assert!(
        !stderr(&out).contains("ITEM_LOST"),
        "the declared move must pair the items:\n{}",
        stderr(&out)
    );
}

#[test]
fn declared_move_with_a_changed_signature_is_clean_when_excused() {
    let repo = ConsRepo::two_commit("conservation/declared_moves_sig");
    let delta = format!(
        "{}\n\
         [[changed]]\n\
         path = \"crates/fix/src/a.rs\"\n\
         item = \"fix::lib::a::take\"\n\
         reason = \"re-qualified types\"\n\
         owner = \"@rust-guard\"\n",
        moved_take_entry(&repo.base_sha)
    );
    assert_clean(&repo.compare_with_delta(&delta));
}

#[test]
fn unchanged_items_of_sibling_targets_stay_paired() {
    // Both integration-test targets define `N`, `helper` and `works`; all
    // of them share the logical path `fix::test::*`. Nothing but a lib
    // comment changed, so the comparison must be clean.
    // Counterfactual: pairing by the logical identity alone sees two head
    // candidates per base item, pairs nothing and reports every item as
    // ITEM_LOST / TEST_LOST.
    let repo = ConsRepo::two_commit("conservation/dup_unchanged_targets");
    assert_clean(&repo.compare());
}

#[test]
fn unchanged_cfg_twins_stay_paired() {
    // `plat` and `hook` exist twice under one logical path (cfg variants).
    // Counterfactual: the identity key is ambiguous, so the old pairing
    // dropped the head items and reported the base items as ITEM_LOST.
    let repo = ConsRepo::two_commit("conservation/dup_cfg_twins");
    assert_clean(&repo.compare());
}

#[test]
fn moved_twins_pair_one_to_one_and_feed_back_to_clean() {
    // `make` exists twice with an identical body (a::make, b::make) and
    // moves to c::make and d::make. Equal group sizes make the twins
    // interchangeable, so both moves are suggested and a delta made of the
    // suggestions is clean.
    // Counterfactual: without the twin pass the two same-key head
    // candidates are UNPAIRABLE_DUPLICATE and nothing is suggested.
    let repo = ConsRepo::two_commit("conservation/twin_moves");
    let out = repo.compare_with_suggest("suggested-twins.toml");
    let err = stderr(&out);
    assert!(!out.status.success(), "undeclared moves must fail:\n{err}");
    assert!(
        !err.contains("UNPAIRABLE_DUPLICATE"),
        "twins must pair:\n{err}"
    );
    let text = repo.read("suggested-twins.toml");
    assert_eq!(
        text.matches("[[moved]]").count(),
        2,
        "expected both twin moves:\n{text}"
    );
    assert_clean(&repo.compare_with_delta_file("suggested-twins.toml"));
}

#[test]
fn base_only_package_is_a_loss_not_silence() {
    // Counterfactual: discovery used the WORKING tree's cargo metadata,
    // so the base-only package (`crates/gone`, deleted at head) never
    // entered the base inventory and its items vanished silently. With
    // manifest-based discovery from each pinned revision they are
    // ITEM_LOST findings naming their logical paths.
    let repo = ConsRepo::two_commit("conservation/gone_pkg");
    let out = repo.compare();
    let err = stderr(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(err.contains("ITEM_LOST"), "expected ITEM_LOST:\n{err}");
    assert!(
        err.contains("gone::lib::Artifact") && err.contains("gone::lib::stash"),
        "expected losses from the deleted package:\n{err}"
    );
    // The surviving package stays silent.
    assert!(
        !err.contains("fix::lib::Plan"),
        "surviving package must not be flagged:\n{err}"
    );
}

#[test]
fn missing_base_rev_blocks_the_comparison() {
    let repo = ConsRepo::two_commit("conservation/test_lost");
    assert_finding(
        &repo
            .repo
            .run_guard(&["conservation", "--base", "does-not-exist", "--head", "HEAD"]),
        "BLOCKED_COMPARISON",
    );
}

#[test]
fn delta_without_base_sha_is_malformed() {
    // Finding 12: a delta file without `base` is MALFORMED_DELTA, not an
    // unpinned comparison. Reported as a hard error line (not a counted
    // finding), so assert directly instead of via assert_finding.
    let repo = ConsRepo::two_commit("conservation/body_changed");
    let delta = "[[changed]]\n\
                 path = \"crates/fix/src/lib.rs\"\n\
                 item = \"fix::lib::helper\"\n\
                 reason = \"no base pin\"\n\
                 owner = \"@rust-guard\"\n";
    let out = repo.compare_with_delta(delta);
    let err = stderr(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(
        err.contains("MALFORMED_DELTA"),
        "expected MALFORMED_DELTA:\n{err}"
    );
}

#[test]
fn deleted_test_target_is_a_loss() {
    // Base has a test TARGET (`crates/fix/tests/x.rs`); head DELETES the
    // file (the harness overlay removes base files the head fixture does
    // not contain). Its items must surface as TEST_LOST.
    // Counterfactual: fails without per-revision discovery because
    // auto-discovery iterated the CURRENT index (`git ls-files`); in the
    // head commit the file is already gone from the index, so the base
    // inventory never saw the target and the run was clean.
    let repo = ConsRepo::two_commit("conservation/target_deleted");
    assert_finding(&repo.compare(), "TEST_LOST");
}

#[test]
fn path_mounted_test_deletion_is_a_loss() {
    // The test module is mounted via a NAME-VALUE `#[path]` attribute and
    // deleted at head (declaration AND file).
    // Counterfactual: fails without the shared attribute-path extraction
    // because conservation's local `parse_args::<LitStr>()` cannot read
    // `#[path = "..."]`, the module silently fell back to default
    // resolution (which finds nothing), its base items vanished without
    // a word and the run was clean.
    let repo = ConsRepo::two_commit("conservation/path_test_deleted");
    assert_finding(&repo.compare(), "TEST_LOST");
}

#[test]
fn cfg_attr_path_is_rejected_during_conservation_inventory() {
    // `cfg_attr(path)` is unsupported, not default resolution; one structured
    // unsupported-path finding is emitted for each revision.
    let repo = ConsRepo::two_commit("conservation/fallback_changed");
    let output = repo.compare();
    let stderr = stderr(&output);
    assert_eq!(
        stderr.matches("UNSUPPORTED_CFG_ATTR_PATH").count(),
        2,
        "expected one structured finding per revision: {stderr}"
    );
    assert_finding(&output, "UNSUPPORTED_CFG_ATTR_PATH");
    assert!(stderr.contains("UNSUPPORTED_CFG_ATTR_PATH"));
    assert!(!stderr.contains("INVENTORY_ERROR"));
}

#[test]
fn unresolved_child_module_is_an_inventory_error() {
    // `mod ghost;` resolves to nothing at BOTH revisions: fail closed
    // with INVENTORY_ERROR instead of silently skipping the child (its
    // items would silently "vanish" between revisions).
    // Counterfactual: fails without the unresolved-child reporting
    // because the Missing resolution only meant "contributes no items"
    // and the run was clean.
    let repo = ConsRepo::two_commit("conservation/broken_mod");
    assert_finding(&repo.compare(), "INVENTORY_ERROR");
}

#[test]
fn phantom_default_branch_is_not_inventoried() {
    // Unconditional `#[path]` replaces default resolution; this fixture pins
    // the contract that the phantom default file cannot create a lost item.
    // Counterfactual: walking both the explicit path and the default m.rs
    // would add the base-only t_phantom test from m.rs to inventory and thus
    // report TEST_LOST even though the unconditional path selects gen.rs.
    let repo = ConsRepo::two_commit("conservation/uncond_path_default_ignored");
    assert_clean(&repo.compare());
}

#[test]
fn suggest_flag_writes_undeclared_moves() {
    let repo = ConsRepo::two_commit("conservation/suggest_clean_moves");
    let out = repo.compare_with_suggest("suggested.toml");
    let err = stderr(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(err.contains("TEST_LOST"), "expected TEST_LOST:\n{err}");
    assert!(
        err.contains("rust-guard: note: 2 undeclared move(s) written to suggested.toml"),
        "expected the stderr note line:\n{err}"
    );
    // P3 parity: apart from the note line the flagged run's output is
    // byte-identical to the flagless run.
    let plain = repo.compare();
    assert_flagless_output_unchanged(&out, &plain, "rust-guard: note:");
    let text = repo.read("suggested.toml");
    assert!(
        text.contains(&format!("base = \"{}\"", repo.base_sha)),
        "expected the full base sha pin:\n{text}"
    );
    for expected in [
        "path = \"crates/fix/src/lib.rs\"",
        "item = \"fix::lib::helper\"",
        "to_path = \"crates/fix/src/m.rs\"",
        "to_item = \"fix::lib::m::helper\"",
        "item = \"fix::lib::t_ok\"",
        "to_item = \"fix::lib::m::t_ok\"",
    ] {
        assert!(text.contains(expected), "missing `{expected}`:\n{text}");
    }
    for absent in ["t_mut", "gone", "dup", "renamed"] {
        assert!(
            !text.contains(absent),
            "`{absent}` must never be suggested:\n{text}"
        );
    }
}

#[test]
fn suggestion_feeds_back_to_clean() {
    let repo = ConsRepo::two_commit("conservation/suggest_clean_moves");
    let out = repo.compare_with_suggest("suggested.toml");
    assert!(!out.status.success(), "the undeclared moves still fail");
    assert!(repo.repo.root.join("suggested.toml").exists());
    assert_clean(&repo.compare_with_delta_file("suggested.toml"));
}

#[test]
fn mixed_fixture_suggests_only_clean_moves() {
    let repo = ConsRepo::two_commit("conservation/suggest_mixed");
    let out = repo.compare_with_suggest("suggested-mixed.toml");
    let err = stderr(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    // P3 parity: apart from the note line the flagged run's output is
    // byte-identical to the flagless run.
    let plain = repo.compare();
    assert_flagless_output_unchanged(&out, &plain, "rust-guard: note:");
    assert!(err.contains("ITEM_LOST"), "expected ITEM_LOST:\n{err}");
    assert!(
        err.contains("UNPAIRABLE_DUPLICATE"),
        "expected UNPAIRABLE_DUPLICATE:\n{err}"
    );
    let text = repo.read("suggested-mixed.toml");
    assert_eq!(
        text.matches("[[moved]]").count(),
        2,
        "exactly helper and t_ok are clean moves:\n{text}"
    );
    assert!(
        text.contains("fix::lib::helper") && text.contains("fix::lib::t_ok"),
        "expected helper + t_ok entries:\n{text}"
    );
    for absent in ["t_mut", "gone", "dup"] {
        assert!(
            !text.contains(absent),
            "`{absent}` must never be suggested:\n{text}"
        );
    }
}

#[test]
fn without_suggest_flag_no_file_no_note() {
    let repo = ConsRepo::two_commit("conservation/suggest_clean_moves");
    let out = repo.compare();
    let err = stderr(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(
        !err.contains("rust-guard: note:"),
        "no note without the flag:\n{err}"
    );
    assert!(
        !repo.repo.root.join("suggested.toml").exists(),
        "no suggestion file without the flag"
    );
}

#[test]
fn signature_changed_move_is_not_suggested() {
    // P2-1: `helper` moved FILE only (via `#[path]`) while changing its
    // signature (u32 -> i64, body tokens identical). The logical path
    // `fix::lib::m::helper` is UNCHANGED, so the pair lands in the pass-1
    // exact-identity branch at matching.rs:111-124 (ITEM_MOVED). This
    // test FAILS if the `b.signature == h.signature` term is removed
    // from the suggestion predicate — `helper` (pass-1 file move) would
    // then be suggested and the file would exist.
    let repo = ConsRepo::two_commit("conservation/suggest_sig_changed");
    let plain = repo.compare();
    let plain_err = stderr(&plain);
    assert!(
        !plain.status.success(),
        "expected flagless exit != 0:\n{plain_err}"
    );
    assert!(
        plain_err.contains("ITEM_MOVED"),
        "expected ITEM_MOVED for the file-moved item:\n{plain_err}"
    );
    let out = repo.compare_with_suggest("suggested.toml");
    let err = stderr(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert_flagless_output_unchanged(&out, &plain, "rust-guard: note:");
    assert!(
        !repo.read_ok("suggested.toml"),
        "a signature-drifted file move must never be suggested (no file at all):\n{err}"
    );
}

#[test]
fn body_changed_moved_test_regression() {
    // P2-3: `t_mut` moved file AND changed body — it falls apart into a
    // lost test (flagless), is never suggested, and a HAND-WRITTEN
    // [[moved]] delta lets the pairing through but the body drift is
    // reported as BODY_CHANGED (the declaration never silences drift).
    let repo = ConsRepo::two_commit("conservation/suggest_mixed");
    // (a) flagless: the move is undeclared AND drifted -> TEST_LOST.
    let plain = repo.compare();
    let plain_err = stderr(&plain);
    assert!(
        !plain.status.success(),
        "expected flagless exit != 0:\n{plain_err}"
    );
    assert!(
        plain_err
            .lines()
            .any(|l| l.contains("TEST_LOST") && l.contains("fix::lib::t_mut")),
        "expected TEST_LOST naming fix::lib::t_mut:\n{plain_err}"
    );
    assert!(
        plain_err
            .lines()
            .any(|l| l.contains("TEST_LOST") && l.contains("fix::lib::t_ok")),
        "expected TEST_LOST naming fix::lib::t_ok:\n{plain_err}"
    );
    let out = repo.compare_with_suggest("suggested.toml");
    assert!(!out.status.success(), "expected exit != 0");
    assert_flagless_output_unchanged(&out, &plain, "rust-guard: note:");
    if repo.read_ok("suggested.toml") {
        let text = repo.read("suggested.toml");
        assert!(
            !text.contains("t_mut"),
            "a body-drifted move must never be suggested:\n{text}"
        );
    }
    // (b) hand-written delta declaring ONLY the t_mut move: the pairing
    // succeeds, the body change surfaces as BODY_CHANGED.
    let delta = format!(
        "base = \"{}\"\n\n\
         [[moved]]\n\
         path = \"{LIB}\"\n\
         item = \"fix::lib::t_mut\"\n\
         to_path = \"crates/fix/src/m.rs\"\n\
         to_item = \"fix::lib::m::t_mut\"\n\
         reason = \"hand-declared move of the drifted test\"\n\
         owner = \"@rust-guard\"\n",
        repo.base_sha
    );
    repo.write("delta-body.toml", &delta);
    let out = repo.compare_with_delta_file("delta-body.toml");
    let err = stderr(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(
        err.contains("BODY_CHANGED"),
        "the declared move must still report the body drift:\n{err}"
    );
}

#[test]
fn suggest_flag_leaves_failing_fixture_output_unchanged() {
    // P3: a fixture that produces NO suggestions at all — the flagged
    // run is byte-identical to the flagless run: same exit code, same
    // stderr (no note line), and no file written.
    let repo = ConsRepo::two_commit("conservation/body_changed");
    let plain = repo.compare();
    let out = repo.compare_with_suggest("suggested.toml");
    assert_flagless_output_unchanged(&out, &plain, "rust-guard: note:");
    let err = stderr(&out);
    assert!(
        !err.contains("rust-guard: note:"),
        "no note when nothing is suggested:\n{err}"
    );
    assert!(
        !repo.repo.root.join("suggested.toml").exists(),
        "no file when nothing is suggested"
    );
}
