//! Orchestration of the `discovery` gate: argument handling, list
//! loading, delta pairing and the finding report.
//!
//! See the module docs in `mod.rs` for the identity definition, the
//! delta `[[moved]]`/`[[renamed]]` mapping and the newly-ignored
//! limitation.

use super::lists::TestList;
use crate::conservation_cmd::delta;
use crate::finding::{exit_code, Report};
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

    // Pair base tests with head tests through [[moved]]/[[renamed]]:
    // base side (path, item) = (binary id, test name), head side
    // (to_path, to_item) likewise (see mod.rs). The first consumable
    // pairing in deterministic (BTreeMap) order wins.
    let mut lost: Vec<(String, String)> = Vec::new();
    for (base_bin, base_test) in base.counts.keys() {
        let mut paired = false;
        for (head_bin, head_test) in head.counts.keys() {
            if d.consume_pair("moved", base_bin, base_test, head_bin, head_test)
                || d.consume_pair("renamed", base_bin, base_test, head_bin, head_test)
            {
                paired = true;
                break;
            }
        }
        if !paired {
            lost.push((base_bin.clone(), base_test.clone()));
        }
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
