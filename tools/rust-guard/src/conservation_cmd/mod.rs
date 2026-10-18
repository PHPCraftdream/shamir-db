//! `conservation` — source inventory diff between two revisions
//! (CI-03a chunk B).
//!
//! Re-exports only; logic lives in the sibling files.

pub mod delta;
pub mod inventory;
pub mod item;
pub mod matching;

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
    if let Some(expected) = delta.base.clone() {
        if !sha_matches(&base_sha, &expected) {
            report.add(
                &args.base,
                1,
                "BASE_MISMATCH",
                format!("delta expects base {expected}, comparison base resolves to {base_sha}"),
            );
        }
    }

    let mut base_items = inventory::build(&args.base, &args.packages, &args.repo)?;
    let mut head_items = inventory::build(&args.head, &args.packages, &args.repo)?;
    // Split impl blocks merge into one logical member: dedup identical
    // items (same identity + fingerprint + signature + test state).
    dedup(&mut base_items);
    dedup(&mut head_items);

    matching::compare(&base_items, &head_items, &mut delta, &mut report);
    report.print();
    Ok(exit_code(&report))
}

/// Removes fully identical duplicates (same identity, fingerprint,
/// signature, cfg and test state) — e.g. one member declared in two
/// blocks of a split impl is one logical member.
fn dedup(items: &mut Vec<item::Item>) {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
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
    use item::TestKind as K;
    match k {
        K::None => "",
        K::Plain => "t",
        K::Tokio => "T",
        K::PropTest => "p",
        K::Loom => "l",
    }
}

/// Abbreviated or full SHA comparison.
fn sha_matches(actual: &str, expected: &str) -> bool {
    actual == expected || actual.starts_with(expected)
}
