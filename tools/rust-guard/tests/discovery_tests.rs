//! Self-tests for the `discovery` gate (CI-only): the inputs are
//! single JSON documents, so the fixtures are inline strings written
//! into a throwaway repo — no fixture tree is needed for list pairs.

mod harness;

use crate::harness::{assert_clean, assert_finding, TempRepo};
use std::process::Output;

/// Builds one `cargo nextest list --message-format json` document from
/// (binary id, [(test name, ignored)]) suites.
fn doc(suites: &[(&str, &[(&str, bool)])]) -> String {
    let mut s = String::from("{\"rust-build-meta\":{},\"test-count\":0,\"rust-suites\":{");
    for (i, (bin, tests)) in suites.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "\"{bin}\":{{\"binary-id\":\"{bin}\",\"testcases\":{{"
        ));
        for (j, (test, ignored)) in tests.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            s.push_str(&format!("\"{test}\":{{\"ignored\":{ignored}}}",));
        }
        s.push_str("}}");
    }
    s.push_str("}}");
    s
}

/// Single-binary convenience wrapper over `doc`.
fn lines(bin: &str, tests: &[&str]) -> String {
    let owned: Vec<(&str, bool)> = tests.iter().map(|t| (*t, false)).collect();
    doc(&[(bin, &owned)])
}

fn write(repo: &TempRepo, rel: &str, contents: &str) {
    std::fs::write(repo.root.join(rel), contents).expect("write file");
}

/// Throwaway repo with the given list files (absent = not written) and
/// an optional delta, then one `discovery` run against them.
fn run(base: Option<&str>, head: Option<&str>, delta: Option<&str>) -> Output {
    let repo = TempRepo::with_fixture("discovery_none");
    if let Some(b) = base {
        write(&repo, "base.json", b);
    }
    if let Some(h) = head {
        write(&repo, "head.json", h);
    }
    let mut args = vec![
        "discovery",
        "--base-list",
        "base.json",
        "--head-list",
        "head.json",
    ];
    if let Some(d) = delta {
        write(&repo, "delta.toml", d);
        args.extend(["--delta", "delta.toml"]);
    }
    repo.run_guard(&args)
}

fn delta(kind: &str, fields: &[(&str, &str)]) -> String {
    let mut s = String::from("base = \"0\"\n\n");
    s.push_str(&format!("[[{kind}]]\n"));
    for (key, value) in fields {
        s.push_str(&format!("{key} = \"{value}\"\n"));
    }
    s
}

#[test]
fn identical_lists_are_clean() {
    let out = run(
        Some(&lines("shamir-engine", &["a", "b"])),
        Some(&lines("shamir-engine", &["a", "b"])),
        None,
    );
    assert_clean(&out);
}

#[test]
fn head_only_addition_is_clean() {
    let out = run(
        Some(&lines("shamir-engine", &["a"])),
        Some(&lines("shamir-engine", &["a", "added"])),
        None,
    );
    assert_clean(&out);
}

#[test]
fn lost_test_is_a_finding() {
    let out = run(
        Some(&lines("shamir-engine", &["kept", "dropped"])),
        Some(&lines("shamir-engine", &["kept"])),
        None,
    );
    assert_finding(&out, "TEST_LOST");
}

#[test]
fn duplicate_identity_is_a_finding() {
    let dup = doc(&[(
        "shamir-engine",
        &[("dup", false), ("dup", false), ("ok", false)],
    )]);
    let out = run(Some(&lines("shamir-engine", &["ok"])), Some(&dup), None);
    assert_finding(&out, "TEST_DUPLICATE");
}

#[test]
fn lost_binary_is_a_finding() {
    // The whole base binary vanishes; the surviving head test lives in a
    // different binary, so both the target and the identity are lost.
    let out = run(
        Some(&lines("old-bin", &["t"])),
        Some(&lines("new-bin", &["t"])),
        None,
    );
    assert_finding(&out, "TARGET_LOST");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("TEST_LOST"), "expected TEST_LOST too:\n{err}");
}

#[test]
fn empty_head_list_is_a_failure() {
    // A suite with zero testcases: an empty inventory is a failure,
    // never a vacuous pass.
    let out = run(
        Some(&lines("shamir-engine", &["t"])),
        Some(&lines("shamir-engine", &[])),
        None,
    );
    assert_finding(&out, "EMPTY_LIST");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("TEST_LOST"),
        "the lone base test is lost:\n{err}"
    );
}

#[test]
fn empty_base_list_is_a_failure() {
    let out = run(
        Some(&lines("shamir-engine", &[])),
        Some(&lines("shamir-engine", &["t"])),
        None,
    );
    assert_finding(&out, "EMPTY_LIST");
}

#[test]
fn missing_base_file_blocks_the_comparison() {
    // base.json is never written: an explicit BLOCKED_COMPARISON
    // finding, never an empty baseline that would pass vacuously.
    let out = run(None, Some(&lines("shamir-engine", &["t"])), None);
    assert_finding(&out, "BLOCKED_COMPARISON");
}

#[test]
fn unparseable_base_file_blocks_the_comparison() {
    let out = run(Some("this is not json\n"), Some(&doc(&[])), None);
    assert_finding(&out, "BLOCKED_COMPARISON");
}

#[test]
fn base_json_without_rust_suites_blocks_the_comparison() {
    // Valid JSON, wrong shape (legacy JSON-lines object): fail closed.
    let out = run(
        Some("{\"type\":\"test\",\"test\":\"t\",\"binary\":\"b\"}"),
        Some(&lines("b", &["t"])),
        None,
    );
    assert_finding(&out, "BLOCKED_COMPARISON");
}

#[test]
fn declared_move_is_accepted() {
    let d = delta(
        "moved",
        &[
            ("path", "bin_a"),
            ("item", "t"),
            ("to_path", "bin_b"),
            ("to_item", "t"),
            ("reason", "test moved to another binary"),
            ("owner", "@rust-guard"),
        ],
    );
    let out = run(
        Some(&lines("bin_a", &["t"])),
        Some(&lines("bin_b", &["t"])),
        Some(&d),
    );
    assert_clean(&out);
}

#[test]
fn declared_rename_is_accepted() {
    let d = delta(
        "renamed",
        &[
            ("path", "shamir-engine"),
            ("item", "old_name"),
            ("to_item", "new_name"),
            ("reason", "clearer name"),
            ("owner", "@rust-guard"),
        ],
    );
    let out = run(
        Some(&lines("shamir-engine", &["old_name"])),
        Some(&lines("shamir-engine", &["new_name"])),
        Some(&d),
    );
    assert_clean(&out);
}

#[test]
fn undeclared_move_is_a_loss() {
    // The same move as `declared_move_is_accepted` without the delta:
    // the run must NOT pass.
    let out = run(
        Some(&lines("bin_a", &["t"])),
        Some(&lines("bin_b", &["t"])),
        None,
    );
    assert_finding(&out, "TEST_LOST");
}

#[test]
fn move_without_to_fields_is_a_finding() {
    // A declaration with no head side at all: the base test is lost
    // AND the malformed entry is an unused allowance — never silence.
    let d = delta(
        "moved",
        &[
            ("path", "bin_a"),
            ("item", "t"),
            ("reason", "entry forgot its head side"),
            ("owner", "@rust-guard"),
        ],
    );
    let out = run(
        Some(&lines("bin_a", &["t"])),
        Some(&lines("bin_b", &["t"])),
        Some(&d),
    );
    assert_finding(&out, "TEST_LOST");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("UNUSED_ALLOWANCE"),
        "malformed entry must surface:\n{err}"
    );
}

#[test]
fn move_to_absent_target_is_a_finding() {
    let d = delta(
        "moved",
        &[
            ("path", "bin_a"),
            ("item", "t"),
            ("to_path", "bin_b"),
            ("to_item", "t"),
            ("reason", "declared but the target vanished"),
            ("owner", "@rust-guard"),
        ],
    );
    let out = run(
        Some(&lines("bin_a", &["t"])),
        Some(&lines("bin_c", &["t"])),
        Some(&d),
    );
    assert_finding(&out, "TEST_LOST");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("UNUSED_ALLOWANCE"),
        "unconsumable entry must surface:\n{err}"
    );
}

#[test]
fn second_claimant_on_one_head_identity_is_a_finding() {
    // Two base tests declare a move onto the SAME head identity; the
    // first pairing consumes it, the second is a finding.
    let d = format!(
        "{}\n{}",
        delta(
            "moved",
            &[
                ("path", "bin_a"),
                ("item", "t1"),
                ("to_path", "bin_h"),
                ("to_item", "t"),
                ("reason", "first claim"),
                ("owner", "@rust-guard"),
            ],
        ),
        delta(
            "moved",
            &[
                ("path", "bin_b"),
                ("item", "t2"),
                ("to_path", "bin_h"),
                ("to_item", "t"),
                ("reason", "second claim"),
                ("owner", "@rust-guard"),
            ],
        )
    );
    let base = doc(&[("bin_a", &[("t1", false)]), ("bin_b", &[("t2", false)])]);
    let out = run(Some(&base), Some(&lines("bin_h", &["t"])), Some(&d));
    assert_finding(&out, "TEST_LOST");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("already claimed"),
        "second claimant must be named:\n{err}"
    );
}

#[test]
fn leftover_delta_entry_is_unused_allowance() {
    let d = delta(
        "moved",
        &[
            ("path", "bin_a"),
            ("item", "never_there"),
            ("reason", "stale entry"),
            ("owner", "@rust-guard"),
        ],
    );
    let out = run(
        Some(&lines("shamir-engine", &["t"])),
        Some(&lines("shamir-engine", &["t"])),
        Some(&d),
    );
    assert_finding(&out, "UNUSED_ALLOWANCE");
}

#[test]
fn newly_ignored_test_is_a_finding() {
    let base = lines("shamir-engine", &["t"]);
    let head = doc(&[("shamir-engine", &[("t", true)])]);
    let out = run(Some(&base), Some(&head), None);
    assert_finding(&out, "TEST_IGNORED");
}

#[test]
fn unignored_flip_is_not_a_finding() {
    let base = doc(&[("shamir-engine", &[("t", true)])]);
    let head = doc(&[("shamir-engine", &[("t", false)])]);
    let out = run(Some(&base), Some(&head), None);
    assert_clean(&out);
}

#[test]
fn kind_label_is_echoed_into_findings() {
    // --kind is a label only; it must appear in the finding messages so
    // doctest/loom lists diffed in parallel runs are distinguishable.
    let repo = TempRepo::with_fixture("discovery_none");
    write(&repo, "base.json", &lines("shamir-engine", &["t"]));
    write(&repo, "head.json", &lines("shamir-engine", &[]));
    let out = repo.run_guard(&[
        "discovery",
        "--base-list",
        "base.json",
        "--head-list",
        "head.json",
        "--kind",
        "doc",
    ]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(err.contains("kind doc"), "--kind label missing:\n{err}");
}
