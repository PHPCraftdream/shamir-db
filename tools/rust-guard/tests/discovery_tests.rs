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
            s.push_str(&format!(
                "\"{test}\":{{\"ignored\":{ignored},\"filter-match\":{{\"status\":\"matches\"}}}}"
            ));
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
            ("to_path", "shamir-engine"),
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

#[test]
fn declaration_cannot_reuse_a_directly_matched_head_identity() {
    // `b` survives under its own name; renaming `a` onto it must not
    // hide the loss of `a` behind an identity that is already matched.
    let d = delta(
        "renamed",
        &[
            ("path", "bin"),
            ("item", "a"),
            ("to_path", "bin"),
            ("to_item", "b"),
            (
                "reason",
                "claims a survivor that is already matched by name",
            ),
            ("owner", "@rust-guard"),
        ],
    );
    let out = run(
        Some(&lines("bin", &["a", "b"])),
        Some(&lines("bin", &["b"])),
        Some(&d),
    );
    assert_finding(&out, "TEST_LOST");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("UNUSED_ALLOWANCE"),
        "the refused declaration must surface:
{err}"
    );
}

#[test]
fn testcase_without_ignored_blocks_the_comparison() {
    // A producer regression that drops `ignored` must not silently turn
    // ignore-drift detection off.
    let broken = "{\"rust-suites\":{\"b\":{\"testcases\":{\"t\":                  {\"filter-match\":{\"status\":\"matches\"}}}}}}";
    let out = run(Some(broken), Some(&lines("b", &["t"])), None);
    assert_finding(&out, "BLOCKED_COMPARISON");
}

#[test]
fn testcase_without_filter_match_blocks_the_comparison() {
    let broken = "{\"rust-suites\":{\"b\":{\"testcases\":{\"t\":{\"ignored\":false}}}}}";
    let out = run(Some(broken), Some(&lines("b", &["t"])), None);
    assert_finding(&out, "BLOCKED_COMPARISON");
}

#[test]
fn filter_rejected_testcases_are_not_inventory() {
    // A list taken with `-E` names the rejected tests too; they must not
    // count as tests, so their absence at head is not a loss.
    let base = "{\"rust-suites\":{\"b\":{\"testcases\":{                \"kept\":{\"ignored\":false,\"filter-match\":{\"status\":\"matches\"}},                \"other\":{\"ignored\":false,\"filter-match\":{\"status\":\"mismatches\"}}}}}}";
    let out = run(Some(base), Some(&lines("b", &["kept"])), None);
    assert_clean(&out);
}

/// Throwaway repo with the given list files and an optional
/// `--suggest-delta` target, then one `discovery` run against them
/// (the repo is returned so callers can read back the suggestion file
/// and re-run against it). `--base-sha` is REQUIRED with
/// `--suggest-delta` (clap-enforced), so it is always passed.
fn run_suggest(
    base: &str,
    head: &str,
    base_sha: &str,
    suggest: Option<&str>,
) -> (TempRepo, Output) {
    let repo = TempRepo::with_fixture("discovery_none");
    write(&repo, "base.json", base);
    write(&repo, "head.json", head);
    let mut args = vec![
        "discovery",
        "--base-list",
        "base.json",
        "--head-list",
        "head.json",
    ];
    if let Some(p) = suggest {
        args.extend(["--base-sha", base_sha, "--suggest-delta", p]);
    }
    let out = repo.run_guard(&args);
    (repo, out)
}

/// P3 parity: `--suggest-delta` must not change the gate's output apart
/// from the note line(s) — same exit code, identical stdout, and a
/// stderr that is the plain stderr plus note line(s) only.
fn assert_flagless_output_unchanged(flagged: &Output, plain: &Output, note_marker: &str) {
    assert_eq!(
        flagged.status.code(),
        plain.status.code(),
        "exit code changed by --suggest-delta"
    );
    assert_eq!(
        String::from_utf8_lossy(&flagged.stdout),
        String::from_utf8_lossy(&plain.stdout),
        "stdout changed by --suggest-delta"
    );
    let stripped: String = String::from_utf8_lossy(&flagged.stderr)
        .lines()
        .filter(|line| !line.contains(note_marker))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_eq!(
        stripped,
        String::from_utf8_lossy(&plain.stderr),
        "stderr changed by --suggest-delta beyond the note line(s)"
    );
}

fn stderr_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn discovery_suggests_single_name_match() {
    // `t` moved from bin_a to bin_b with no declaration: TEST_LOST, but
    // the head side is unambiguous, so --suggest-delta proposes it.
    let (_repo, out) = run_suggest(
        &lines("bin_a", &["t", "u"]),
        &doc(&[("bin_a", &[("u", false)]), ("bin_b", &[("t", false)])]),
        "0123456789abcdef0123456789abcdef01234567",
        Some("suggested-disc.toml"),
    );
    let err = stderr_text(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(err.contains("TEST_LOST"), "expected TEST_LOST:\n{err}");
    assert!(
        err.contains("rust-guard: note: 1 undeclared move(s) written to suggested-disc.toml"),
        "expected the stderr note line:\n{err}"
    );
    let text =
        std::fs::read_to_string(_repo.root.join("suggested-disc.toml")).expect("read suggestion");
    for expected in [
        "[[moved]]",
        "path = \"bin_a\"",
        "item = \"t\"",
        "to_path = \"bin_b\"",
        "to_item = \"t\"",
        "base = \"0123456789abcdef0123456789abcdef01234567\"",
    ] {
        assert!(text.contains(expected), "missing `{expected}`:\n{text}");
    }
}

#[test]
fn discovery_suggestion_feeds_back_clean() {
    let (repo, out) = run_suggest(
        &lines("bin_a", &["t", "u"]),
        &doc(&[("bin_a", &[("u", false)]), ("bin_b", &[("t", false)])]),
        "0123456789abcdef0123456789abcdef01234567",
        Some("suggested-disc.toml"),
    );
    assert!(!out.status.success(), "the undeclared move still fails");
    assert!(repo.root.join("suggested-disc.toml").exists());
    let clean = repo.run_guard(&[
        "discovery",
        "--base-list",
        "base.json",
        "--head-list",
        "head.json",
        "--delta",
        "suggested-disc.toml",
    ]);
    assert_clean(&clean);
}

#[test]
fn discovery_ambiguous_name_not_suggested() {
    // Two head candidates carry the test name: no pairing can be
    // chosen, so nothing is suggested and no file is written.
    let (repo, out) = run_suggest(
        &lines("bin_a", &["t"]),
        &doc(&[("bin_b", &[("t", false)]), ("bin_c", &[("t", false)])]),
        "0123456789abcdef0123456789abcdef01234567",
        Some("suggested-amb.toml"),
    );
    let err = stderr_text(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(err.contains("TEST_LOST"), "expected TEST_LOST:\n{err}");
    assert!(err.contains("TARGET_LOST"), "expected TARGET_LOST:\n{err}");
    assert!(
        !repo.root.join("suggested-amb.toml").exists(),
        "an ambiguous pairing must never be suggested"
    );
}

#[test]
fn discovery_without_flag_no_file() {
    let (repo, out) = run_suggest(
        &lines("bin_a", &["t", "u"]),
        &doc(&[("bin_a", &[("u", false)]), ("bin_b", &[("t", false)])]),
        "0123456789abcdef0123456789abcdef01234567",
        None,
    );
    let err = stderr_text(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(
        !err.contains("rust-guard: note:"),
        "no note without the flag:\n{err}"
    );
    assert!(
        !repo.root.join("suggested-disc.toml").exists(),
        "no suggestion file without the flag"
    );
}

#[test]
fn discovery_reverse_ambiguity_not_suggested() {
    // Reverse ambiguity: TWO base identities carry the test name and only
    // ONE head candidate exists — the 1:1 assumption is unproven (which
    // base test moved?), so no one gets a suggestion.
    let base = doc(&[("bin_a", &[("t", false)]), ("bin_b", &[("t", false)])]);
    let (repo, out) = run_suggest(
        &base,
        &lines("bin_c", &["t"]),
        "0123456789abcdef0123456789abcdef01234567",
        Some("suggested-rev.toml"),
    );
    let err = stderr_text(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(err.contains("TEST_LOST"), "expected TEST_LOST:\n{err}");
    assert!(
        !repo.root.join("suggested-rev.toml").exists(),
        "a reverse-ambiguous pairing must never be suggested"
    );
}

#[test]
fn discovery_ignored_drift_not_suggested() {
    // The candidate test is `ignored` at head but active at base: the
    // ignored-state drift must stay a finding, never a suggestion.
    let (repo, out) = run_suggest(
        &lines("bin_a", &["t"]),
        &doc(&[("bin_b", &[("t", true)])]),
        "0123456789abcdef0123456789abcdef01234567",
        Some("suggested-ign.toml"),
    );
    let err = stderr_text(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(err.contains("TEST_LOST"), "expected TEST_LOST:\n{err}");
    assert!(
        !repo.root.join("suggested-ign.toml").exists(),
        "an ignored-drifted move must never be suggested (no suggestions -> no file)"
    );
}

#[test]
fn discovery_flagless_output_unchanged() {
    // P3: apart from the note line(s), the flagged run's output is
    // byte-identical to the flagless run.
    let base = &lines("bin_a", &["t", "u"]);
    let head = &doc(&[("bin_a", &[("u", false)]), ("bin_b", &[("t", false)])]);
    let (_repo, flagged) = run_suggest(
        base,
        head,
        "0123456789abcdef0123456789abcdef01234567",
        Some("suggested-parity.toml"),
    );
    let (_repo2, plain) = run_suggest(base, head, "unused", None);
    assert_flagless_output_unchanged(&flagged, &plain, "rust-guard: note:");
}

#[test]
fn discovery_suggestion_escapes_special_characters() {
    // P2-5: TOML basic-string escaping. The test name carries a quote, a
    // backslash and a control byte; the suggestion file must escape them
    // (Rust's `{:?}` renders `\u{7}` which is INVALID TOML), and feeding
    // the file back as `--delta` must round-trip the identity exactly.
    // In the JSON builder string the name is JSON-escaped:
    // we"ird\test<U+0007>x -> we\"ird\\test\u0007x
    let weird = "we\\\"ird\\\\test\\u0007x";
    let base = lines("bin_a", &[weird]);
    let head = lines("bin_b", &[weird]);
    let (repo, out) = run_suggest(&base, &head, "deadbeef", Some("suggested-esc.toml"));
    let err = stderr_text(&out);
    assert!(!out.status.success(), "expected exit != 0:\n{err}");
    assert!(err.contains("TEST_LOST"), "expected TEST_LOST:\n{err}");
    assert!(
        err.contains("rust-guard: note: 1 undeclared move(s) written to suggested-esc.toml"),
        "expected the stderr note line:\n{err}"
    );
    let text =
        std::fs::read_to_string(repo.root.join("suggested-esc.toml")).expect("read suggestion");
    assert!(
        text.contains("\\u0007"),
        "the control byte must be \\u-escaped:\n{text}"
    );
    assert!(
        !text.contains('\u{7}'),
        "the raw control byte must not appear:\n{text}"
    );
    // Feed the suggestion back: it must parse (no MALFORMED_DELTA) and
    // the escaped strings must round-trip exactly (the [[moved]] entry
    // is only consumed on an exact path/item match), making the run clean.
    let clean = repo.run_guard(&[
        "discovery",
        "--base-list",
        "base.json",
        "--head-list",
        "head.json",
        "--delta",
        "suggested-esc.toml",
    ]);
    let clean_err = stderr_text(&clean);
    assert!(
        !clean_err.contains("MALFORMED_DELTA"),
        "the suggestion file must be valid TOML:\n{clean_err}"
    );
    assert_clean(&clean);
}

#[test]
fn discovery_flag_leaves_no_suggestion_fixture_output_unchanged() {
    // P3: a fixture that produces NO suggestions — the flagged run is
    // byte-identical to the flagless run (no note line, no file).
    let base = &lines("bin_a", &["t"]);
    let head = &doc(&[("bin_b", &[("t", false)]), ("bin_c", &[("t", false)])]);
    let (repo, flagged) = run_suggest(
        base,
        head,
        "0123456789abcdef0123456789abcdef01234567",
        Some("suggested-parity.toml"),
    );
    let (_repo2, plain) = run_suggest(base, head, "unused", None);
    assert_flagless_output_unchanged(&flagged, &plain, "rust-guard: note:");
    let err = stderr_text(&flagged);
    assert!(
        !err.contains("rust-guard: note:"),
        "no note when nothing is suggested:\n{err}"
    );
    assert!(
        !repo.root.join("suggested-parity.toml").exists(),
        "no file when nothing is suggested"
    );
}
