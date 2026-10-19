//! Self-tests for the `discovery` gate (CI-only): the inputs are
//! JSON-lines text, so the fixtures are inline strings written into a
//! throwaway repo — no fixture tree is needed for list pairs.

mod harness;

use crate::harness::{assert_clean, assert_finding, TempRepo};
use std::process::Output;

/// Builds a `cargo nextest list --message-format json` shape: one
/// `binary` line then one `test` line per test (binary id inline).
fn lines(bin: &str, tests: &[&str]) -> String {
    let mut s = format!("{{\"type\":\"binary\",\"binary-id\":\"{bin}\"}}\n");
    for t in tests {
        s.push_str(&format!(
            "{{\"type\":\"test\",\"test\":\"{t}\",\"binary\":\"{bin}\"}}\n"
        ));
    }
    s
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
    let dup = "{\"type\":\"test\",\"test\":\"dup\",\"binary\":\"shamir-engine\"}\n\
               {\"type\":\"test\",\"test\":\"dup\",\"binary\":\"shamir-engine\"}\n";
    let out = run(
        Some(&format!("{}{}", lines("shamir-engine", &["ok"]), dup)),
        Some(dup),
        None,
    );
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
    // A binary line with zero test lines: an empty inventory is a
    // failure, never a vacuous pass.
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
    let out = run(Some("this is not json\n"), Some("{}\n"), None);
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
fn identical_lists_are_clean() {
    let out = run(
        Some(&lines("shamir-engine", &["a", "b"])),
        Some(&lines("shamir-engine", &["a", "b"])),
        None,
    );
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
