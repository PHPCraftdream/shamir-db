//! Orchestration of the `discovery` gate: argument handling, list
//! loading, delta pairing and the finding report.
//!
//! See the module docs in `mod.rs` for the identity definition and the
//! delta `[[moved]]`/`[[renamed]]` mapping.

use super::lists::TestList;
use crate::common::finding::{exit_code, Report};
use crate::conservation_cmd::delta;
use std::collections::BTreeSet;

/// CLI arguments for `rust-guard discovery` (declared in `main.rs`).
pub struct DiscoveryArgs {
    /// Path to the base side list (`cargo nextest list --message-format
    /// json` at the merge base).
    pub base_list: String,
    /// Path to the head side list (same format, current tree).
    pub head_list: String,
    /// TOML delta file; `[[moved]]`/`[[renamed]]` entries pair base
    /// tests with head tests (optional; without it every lost test is a
    /// finding).
    pub delta: Option<String>,
    /// Label (`lib|loom|doc`) echoed into finding paths/messages so
    /// doctest/loom lists can be diffed separately.
    pub kind: String,
}

/// Runs the gate; returns the process exit code.
pub fn run(args: DiscoveryArgs) -> u8 {
    match execute(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rust-guard: error: {e}");
            1
        }
    }
}

fn execute(args: DiscoveryArgs) -> Result<u8, String> {
    let mut report = Report::new();
    // Blocked comparison: a missing/unreadable/unparseable list is an
    // explicit finding, never an empty baseline.
    let Some(base) = read_list(&args.base_list, "base", &mut report) else {
        report.print();
        return Ok(exit_code(&report));
    };
    let Some(head) = read_list(&args.head_list, "head", &mut report) else {
        report.print();
        return Ok(exit_code(&report));
    };

    // A list with zero tests is a failure, not a pass.
    if base.counts.is_empty() {
        report.add(
            &args.base_list,
            1,
            "EMPTY_LIST",
            format!("base list (kind {}) holds zero tests", args.kind),
        );
    }
    if head.counts.is_empty() {
        report.add(
            &args.head_list,
            1,
            "EMPTY_LIST",
            format!("head list (kind {}) holds zero tests", args.kind),
        );
    }
    for identity in base.duplicates() {
        report.add(
            &args.base_list,
            1,
            "TEST_DUPLICATE",
            format!(
                "identity {}::{} appears more than once in the base list",
                identity.0, identity.1
            ),
        );
    }
    for identity in head.duplicates() {
        report.add(
            &args.head_list,
            1,
            "TEST_DUPLICATE",
            format!(
                "identity {}::{} appears more than once in the head list",
                identity.0, identity.1
            ),
        );
    }

    let mut d = delta::load(args.delta.as_deref())?;

    // Pass 1 — direct matches: an identity present in BOTH base and
    // head needs no delta. Only here is a false->true `ignored` flip a
    // TEST_IGNORED finding (a test that travelled through a declared
    // move/rename is not re-judged).
    let mut moved_out: Vec<(String, String)> = Vec::new();
    for identity in base.counts.keys() {
        match head.counts.get(identity) {
            Some(_) => {
                if !base.ignored.get(identity).copied().unwrap_or(false)
                    && head.ignored.get(identity).copied().unwrap_or(false)
                {
                    report.add(
                        &args.base_list,
                        1,
                        "TEST_IGNORED",
                        format!(
                            "test {}::{} (kind {}) is newly `ignored` in the head list",
                            identity.0, identity.1, args.kind
                        ),
                    );
                }
            }
            None => moved_out.push((identity.0.clone(), identity.1.clone())),
        }
    }

    // Pass 2 — declared moves/renames for every base identity ABSENT
    // from head. The declaration must name a head identity
    // (to_path/to_item) that actually exists; each head identity may be
    // consumed by at most ONE declaration.
    let mut lost: Vec<(String, String)> = Vec::new();
    let mut claimed_heads: BTreeSet<(String, String)> = BTreeSet::new();
    for (base_bin, base_test) in &moved_out {
        let entry = take_declaration(&mut d, base_bin, base_test);
        let Some(entry) = entry else {
            lost.push((base_bin.clone(), base_test.clone()));
            continue;
        };
        let Some((head_bin, head_test)) = &entry.to_path.clone().zip(entry.to_item.clone()) else {
            // Malformed entry: no head side declared at all. The base
            // test is lost AND the entry itself is reported as an
            // unused allowance below (it stays unconsumed).
            report.add(
                &args.base_list,
                1,
                "TEST_LOST",
                format!(
                    "test {base_bin}::{base_test} (kind {}) has a {} declaration for \
                     {}:{} without to_path/to_item and no head counterpart",
                    args.kind, entry.kind, entry.path, entry.item,
                ),
            );
            lost.push((base_bin.clone(), base_test.clone()));
            continue;
        };
        if !head
            .counts
            .contains_key(&(head_bin.clone(), head_test.clone()))
        {
            // Declared target does not exist at head: the base test is
            // lost and the entry stays unconsumed (unused allowance).
            report.add(
                &args.base_list,
                1,
                "TEST_LOST",
                format!(
                    "test {base_bin}::{base_test} (kind {}) declares a move/rename to \
                     {head_bin}::{head_test}, which does not exist in the head list",
                    args.kind,
                ),
            );
            lost.push((base_bin.clone(), base_test.clone()));
            continue;
        }
        let head_identity = (head_bin.clone(), head_test.clone());
        if !claimed_heads.insert(head_identity) {
            report.add(
                &args.base_list,
                1,
                "TEST_LOST",
                format!(
                    "test {base_bin}::{base_test} (kind {}) declares a move/rename to \
                     {head_bin}::{head_test}, which is already claimed by another base test",
                    args.kind,
                ),
            );
            lost.push((base_bin.clone(), base_test.clone()));
            continue;
        }
        // The entry was already removed from the delta by
        // `take_declaration`; dropping it here consumes it, so the
        // leftover sweep below never reports it as an unused allowance.
    }
    for (bin, test) in &lost {
        report.add(
            &args.base_list,
            1,
            "TEST_LOST",
            format!(
                "test {bin}::{test} (kind {}) has no head counterpart and \
                 no [[moved]]/[[renamed]] declaration",
                args.kind
            ),
        );
    }

    // A base binary is lost when every one of its remaining unpaired
    // tests is gone AND the head list carries no tests for it (a
    // declared move of the binary's last test to another binary is
    // legal and must not double-report).
    let head_bins = head.binaries();
    let lost_bins: BTreeSet<&String> = lost
        .iter()
        .map(|(bin, _)| bin)
        .filter(|bin| !head_bins.contains(bin.as_str()))
        .collect();
    for bin in lost_bins {
        report.add(
            &args.base_list,
            1,
            "TARGET_LOST",
            format!("binary/target `{bin}` (kind {kind}) exists in the base list and not in the head list", kind = args.kind),
        );
    }

    // Whatever the pairing could not consume is an unused allowance.
    for entry in d.all() {
        report.add(
            &args.base_list,
            1,
            "UNUSED_ALLOWANCE",
            format!(
                "delta entry {}:{} (reason: {}; owner: {}) matched no test pairing",
                entry.path, entry.item, entry.reason, entry.owner
            ),
        );
    }

    report.print();
    Ok(exit_code(&report))
}

/// Removes and returns the first `[[moved]]`/`[[renamed]]` entry whose
/// base side (`path` + `item`) matches, tagging it with its kind.
fn take_declaration(d: &mut delta::Delta, bin: &str, test: &str) -> Option<ClaimedEntry> {
    for kind in ["moved", "renamed"] {
        let list = d.list_mut(kind);
        if let Some(pos) = list.iter().position(|e| e.path == bin && e.item == test) {
            let entry = list.remove(pos);
            return Some(ClaimedEntry {
                kind: kind.to_owned(),
                path: entry.path,
                item: entry.item,
                to_path: entry.to_path,
                to_item: entry.to_item,
            });
        }
    }
    None
}

/// A `[[moved]]`/`[[renamed]]` declaration taken out of the delta,
/// pending validation of its head side.
struct ClaimedEntry {
    kind: String,
    path: String,
    item: String,
    to_path: Option<String>,
    to_item: Option<String>,
}

/// Reads and parses one list file; a failure is a `BLOCKED_COMPARISON`
/// finding on that side (and `None`, short-circuiting the comparison).
fn read_list(path: &str, side: &str, report: &mut Report) -> Option<TestList> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            report.add(
                path,
                1,
                "BLOCKED_COMPARISON",
                format!("{side} list cannot be read: {e}"),
            );
            return None;
        }
    };
    match TestList::parse(&text) {
        Ok(list) => Some(list),
        Err(e) => {
            report.add(
                path,
                1,
                "BLOCKED_COMPARISON",
                format!("{side} list cannot be parsed: {e}"),
            );
            None
        }
    }
}
