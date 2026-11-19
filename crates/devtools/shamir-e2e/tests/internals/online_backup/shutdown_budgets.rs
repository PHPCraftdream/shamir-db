//! Quiet-shutdown byte-budget scan (brief 50-r2a-4 follow-up 5): after the
//! scenario's two clean shutdowns, every regular file under a data dir is
//! classified into four MUTUALLY EXCLUSIVE categories and checked against
//! the aggregate and WAL budgets. A failing scan formats a report — both
//! measured totals and limits, all four categories, the regular-file
//! count and the ten largest files (relative paths, logical lengths via
//! `symlink_metadata`, symlinks never followed) — as an `Option<String>`;
//! a scan within budget formats nothing. Absolute paths never leak: scan
//! errors carry only the relative path (`.` for the root) and the actual
//! I/O error.
//!
//! Budget arithmetic (CI observed 554,143,135 bytes on Ubuntu source):
//! total budget = 8 stores × 64 MiB journals + 16 exact pad + 32 WAL
//! allowance + 16 overhead MiB. Fjall 3.1.13 preallocates EVERY new
//! journal at 64 MiB (`set_len`, Drop syncs without shrinking, rotation
//! only past a 64,000,000 writer position); the 32 MiB WAL term is a
//! conservative aggregate allowance (2 repo dirs × (1 active + 1
//! sealed/sidecar slack slot) × 8 MiB default Shamir WAL rotation
//! threshold — the Shamir WAL is not preallocated), NOT a segment
//! contract; the 16 MiB overhead term exceeds the observed 495,007-byte
//! residual by >30×. The 512 MiB journal-pressure threshold is a pressure
//! trigger, not an actual allocation, and needs no extra term.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The eight store roots: `server_meta`, `users`, `counters`,
/// `audit_checkpoint` (launch.rs:147–172, audit_appender.rs:38–43),
/// `api_tokens` (launch.rs:537, tokens/store.rs:56–66), the catalogue
/// `shamir_db_meta.redb` and the boot default repo
/// `shamir_db_default_main.redb` (launch.rs:407/408), and the
/// fixture-created workload repo `backup_load/main`
/// (BoxRepoFactory::fjall_raw → root.join(db).join(repo), no extension).
/// Classification is component-wise `Path::starts_with`, so exactly the
/// `backup_load/main` subtree matches — never all of `backup_load`; the
/// flat `audit.log` file falls under `other`.
const STORE_ROOTS: &[&str] = &[
    "server_meta",
    "users",
    "counters",
    "audit_checkpoint",
    "api_tokens",
    "shamir_db_meta.redb",
    "shamir_db_default_main.redb",
    "backup_load/main",
];

/// Total budget in MiB: 8 stores × 64 MiB journals + 16 exact pad + 32
/// WAL allowance + 16 overhead (see the module header for provenance).
const TOTAL_BUDGET_MIB: u64 = 8 * 64 + 16 + 32 + 16;
const DATA_DIR_TOTAL_BUDGET: u64 = TOTAL_BUDGET_MIB * 1024 * 1024;
/// WAL budget: an aggregate allowance for two repo WAL dirs, not a
/// segment contract.
const WAL_RETAINED_BUDGET: u64 = 32 * 1024 * 1024;

/// How many largest files the failure report carries.
const TOP_FILES: usize = 10;

/// One recursive-scan outcome: the total plus the four MUTUALLY EXCLUSIVE
/// full regular-file byte categories (`other` completes the total), the
/// regular-file count and the bounded ten-largest list (size, relative
/// path — largest first, ties broken deterministically by ascending
/// relative path).
pub(super) struct DirScan {
    total_bytes: u64,
    wal_bytes: u64,
    store_bytes: u64,
    /// Exact canonical pad category total, read by the gate wrapper's
    /// (unchanged) exact-padding assertion.
    pub(super) fixture_bytes: u64,
    other_bytes: u64,
    file_count: u64,
    largest: Vec<(u64, PathBuf)>,
}

/// Relative-path text for error messages: `.` for the scan root, the
/// native path otherwise (never a lossy conversion of a file name).
fn relative_display(relative: &Path) -> String {
    if relative.as_os_str().is_empty() {
        String::from(".")
    } else {
        relative.display().to_string()
    }
}

/// A directory name ends in `.shamirwal`, INCLUDING the bare name
/// `.shamirwal` (which `Path::extension` would miss) — byte suffix on the
/// native `OsStr` encoding, no lossy string conversion.
fn is_wal_dir_name(name: &OsStr) -> bool {
    name.as_encoded_bytes().ends_with(b".shamirwal")
}

/// Component-wise native classification of the relative path as one of
/// the eight store roots.
fn is_store_root(relative: &Path) -> bool {
    STORE_ROOTS
        .iter()
        .any(|root| relative.starts_with(Path::new(root)))
}

/// Recursively classify regular files under `dir` into `scan`. `relative`
/// is the path of `dir` relative to the data dir root. A file is WAL
/// bytes when ANY path component is a `.shamirwal` directory (all files
/// inside count). The canonical pad is ONLY the exact `canonical_pad`
/// path; a same-named stray file elsewhere classifies as `store`/`other`,
/// never as the pad. Files are NEVER removed; symlinks are not followed
/// (metadata only); I/O errors fail the scan (a certification failure)
/// reporting only the relative path (`.` for the root) and the actual
/// error; depth is guarded.
fn scan_dir(
    dir: &Path,
    relative: &Path,
    under_wal: bool,
    canonical_pad: &Path,
    scan: &mut DirScan,
    depth: u32,
) {
    assert!(
        depth < 32,
        "directory scan depth runaway at {}",
        relative_display(relative)
    );
    let entries = dir
        .read_dir()
        .unwrap_or_else(|e| panic!("scan dir {}: {e}", relative_display(relative)));
    for entry in entries {
        let entry =
            entry.unwrap_or_else(|e| panic!("scan entry in {}: {e}", relative_display(relative)));
        let name = entry.file_name();
        let path = dir.join(&name);
        let child_relative = relative.join(&name);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) => panic!("scan metadata {} : {e}", relative_display(&child_relative)),
        };
        if meta.is_dir() {
            // Inherited flag, plus a `.shamirwal` directory name.
            let child_wal = under_wal || is_wal_dir_name(&name);
            scan_dir(
                &path,
                &child_relative,
                child_wal,
                canonical_pad,
                scan,
                depth + 1,
            );
        } else if meta.is_file() {
            let size = meta.len();
            scan.total_bytes = scan.total_bytes.saturating_add(size);
            scan.file_count = scan.file_count.saturating_add(1);
            if under_wal {
                scan.wal_bytes = scan.wal_bytes.saturating_add(size);
            } else if child_relative == canonical_pad {
                scan.fixture_bytes = scan.fixture_bytes.saturating_add(size);
            } else if is_store_root(&child_relative) {
                scan.store_bytes = scan.store_bytes.saturating_add(size);
            } else {
                scan.other_bytes = scan.other_bytes.saturating_add(size);
            }
            scan.largest.push((size, child_relative));
            scan.largest
                .sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            scan.largest.truncate(TOP_FILES);
        }
    }
}

/// Scan `data_dir` (relative classification against `canonical_pad`, the
/// pad file's path relative to the data dir) and return the categorized
/// scan, so the caller can run its own (unchanged) exact-padding
/// assertion without a second walk.
pub(super) fn scan_quiet_shutdown(data_dir: &Path, canonical_pad: &Path) -> DirScan {
    let mut scan = DirScan {
        total_bytes: 0,
        wal_bytes: 0,
        store_bytes: 0,
        fixture_bytes: 0,
        other_bytes: 0,
        file_count: 0,
        largest: Vec::new(),
    };
    // The scan root itself is not under any WAL directory.
    scan_dir(data_dir, Path::new(""), false, canonical_pad, &mut scan, 0);
    scan
}

/// The budget report for a scan that exceeded the aggregate or WAL
/// budget — both measured totals and limits, all four exclusive
/// categories, the regular-file count and the ten largest files — or
/// `None` when the scan is within budget (nothing formatted).
pub(super) fn budget_failure(scan: &DirScan, what: &str) -> Option<String> {
    if scan.total_bytes <= DATA_DIR_TOTAL_BUDGET && scan.wal_bytes <= WAL_RETAINED_BUDGET {
        return None;
    }
    let largest = scan
        .largest
        .iter()
        .enumerate()
        .map(|(idx, (size, path))| format!("  {}. {} ({} bytes)", idx + 1, path.display(), size))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "{what}: data dir total {} bytes (limit {DATA_DIR_TOTAL_BUDGET}), \
         retained WAL {} bytes (limit {WAL_RETAINED_BUDGET}); \
         wal={} store={} fixture_pad={} other={}; regular_files={}; ten largest:\n{largest}",
        scan.total_bytes,
        scan.wal_bytes,
        scan.wal_bytes,
        scan.store_bytes,
        scan.fixture_bytes,
        scan.other_bytes,
        scan.file_count,
    ))
}
