//! Orchestration of the `conservation` gate: argument handling,
//! revision opening, delta validation and the finding report.

use super::delta;
use super::inventory;
use super::item;
use super::matching;
use crate::finding::{exit_code, Report};
use crate::gitfs::GitFs;

/// CLI arguments for `rust-guard conservation` (declared in `main.rs`).
pub struct ConservationArgs {
    /// Repository root (the `--repo` flag).
    pub repo: std::path::PathBuf,
    pub base: String,
    pub head: String,
    pub packages: Vec<String>,
    pub delta: Option<String>,
}

/// Runs the gate; returns the process exit code.
pub fn run(args: ConservationArgs) -> u8 {
    match execute(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rust-guard: error: {e}");
            1
        }
    }
}

fn execute(args: ConservationArgs) -> Result<u8, String> {
    // Blocked comparison: a missing base is never an empty baseline.
    let base_fs = GitFs::open(Some(&args.base), &args.repo)?;
    let Some(base_sha) = base_fs.commit_sha(&args.base) else {
        let mut report = Report::new();
        report.add(
            &args.base,
            1,
            "BLOCKED_COMPARISON",
            format!("base revision `{}` does not name a commit", args.base),
        );
        report.print();
        return Ok(1);
    };
    let head_fs = GitFs::open(Some(&args.head), &args.repo)?;
    if head_fs.commit_sha(&args.head).is_none() {
        let mut report = Report::new();
        report.add(
            &args.head,
            1,
            "BLOCKED_COMPARISON",
            format!("head revision `{}` does not name a commit", args.head),
        );
        report.print();
        return Ok(1);
    }

    let mut delta = delta::load(args.delta.as_deref())?;
    let mut report = Report::new();
    if let Some(expected) = delta.base.as_deref() {
        if !sha_matches(&base_sha, expected) {
            report.add(
                &args.base,
                1,
                "BASE_MISMATCH",
                format!("delta expects base {expected}, comparison base resolves to {base_sha}"),
            );
        }
    }

    let mut base_inv = inventory::build(&args.base, &args.packages, &args.repo)?;
    let mut head_inv = inventory::build(&args.head, &args.packages, &args.repo)?;
    // Fail closed: every unreadable/unparseable source or manifest problem
    // is a finding, never a silent gap in the inventory.
    for (path, msg) in base_inv.errors.drain(..).chain(head_inv.errors.drain(..)) {
        let code = if msg.starts_with("UNSUPPORTED_CFG_ATTR_PATH:") {
            "UNSUPPORTED_CFG_ATTR_PATH"
        } else {
            "INVENTORY_ERROR"
        };
        let message = msg
            .strip_prefix("UNSUPPORTED_CFG_ATTR_PATH: ")
            .unwrap_or(&msg)
            .to_owned();
        report.add(&path, 1, code, message);
    }
    // Split impl blocks merge into one logical member: dedup identical
    // items (same identity + fingerprint + signature + test state).
    dedup(&mut base_inv.items);
    dedup(&mut head_inv.items);

    matching::compare(&base_inv.items, &head_inv.items, &mut delta, &mut report);
    report.print();
    Ok(exit_code(&report))
}

/// Removes fully identical duplicates (same identity, fingerprint,
/// signature, cfg and test state) — e.g. one member declared in two
/// blocks of a split impl is one logical member.
fn dedup(items: &mut Vec<item::Item>) {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    items.retain(|it| {
        let k = [
            item::identity(it),
            it.fingerprint.clone(),
            it.signature.clone(),
            it.cfg.clone().unwrap_or_default(),
            test_kind_tag(&it.test.kind).to_owned(),
            it.test.ignored.to_string(),
        ]
        .join("\u{1}");
        seen.insert(k)
    });
}

fn test_kind_tag(k: &item::TestKind) -> &'static str {
    match k {
        item::TestKind::None => "",
        item::TestKind::Plain => "t",
        item::TestKind::Tokio => "T",
        item::TestKind::PropTest => "p",
        item::TestKind::Loom => "l",
    }
}

/// Abbreviated or full SHA comparison.
fn sha_matches(actual: &str, expected: &str) -> bool {
    actual == expected || actual.starts_with(expected)
}
