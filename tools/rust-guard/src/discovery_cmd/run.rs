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
    // (to_path/to_item) that exists and is not yet accounted for: every
    // direct match already claims its head identity, and each head
    // identity may be consumed by at most ONE declaration. An entry that
    // cannot be honoured stays in the delta (reported as an unused
    // allowance below) and the base test is lost.
    let mut lost: Vec<(String, String)> = Vec::new();
    let mut unexplained: Vec<(String, String)> = Vec::new();
    let mut claimed_heads: BTreeSet<(String, String)> = base
        .counts
        .keys()
        .filter(|identity| head.counts.contains_key(*identity))
        .cloned()
        .collect();
    for (base_bin, base_test) in &moved_out {
        let Some((kind, pos)) = find_declaration(&mut d, base_bin, base_test) else {
            lost.push((base_bin.clone(), base_test.clone()));
            unexplained.push((base_bin.clone(), base_test.clone()));
            continue;
        };
        let entry = d.list_mut(kind)[pos].clone();
        let problem = match (&entry.to_path, &entry.to_item) {
            (Some(to_path), Some(to_item)) => {
                let head_identity = (to_path.clone(), to_item.clone());
                if !head.counts.contains_key(&head_identity) {
                    Some(format!(
                        "declares a move/rename to {to_path}::{to_item}, which does not exist in the head list"
                    ))
                } else if !claimed_heads.insert(head_identity) {
                    Some(format!(
                        "declares a move/rename to {to_path}::{to_item}, which is already claimed by a direct match or another declaration"
                    ))
                } else {
                    None
                }
            }
            _ => Some(format!(
                "has a {kind} declaration for {}:{} without to_path/to_item and no head counterpart",
                entry.path, entry.item
            )),
        };
        lost.push((base_bin.clone(), base_test.clone()));
        match problem {
            None => {
                // Consumed: the leftover sweep below must not report it.
                d.list_mut(kind).remove(pos);
                lost.pop();
            }
            Some(message) => report.add(
                &args.base_list,
                1,
                "TEST_LOST",
                format!(
                    "test {base_bin}::{base_test} (kind {}) {message}",
                    args.kind
                ),
            ),
        }
    }
    for (bin, test) in &unexplained {
        report.add(
            &args.base_list,
            1,
            "TEST_LOST",
            format!(
                "test {bin}::{test} (kind {}) has no head counterpart and no [[moved]]/[[renamed]] declaration",
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

/// Locates the first `[[moved]]`/`[[renamed]]` entry whose base side
/// (`path` + `item`) matches; the entry stays in the delta until the
/// caller has validated and consumed it.
fn find_declaration(d: &mut delta::Delta, bin: &str, test: &str) -> Option<(&'static str, usize)> {
    for kind in ["moved", "renamed"] {
        if let Some(pos) = d
            .list_mut(kind)
            .iter()
            .position(|e| e.path == bin && e.item == test)
        {
            return Some((kind, pos));
        }
    }
    None
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
