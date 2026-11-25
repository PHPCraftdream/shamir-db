//! Owner-side progress attribution for a finished online backup (moved out
//! of `registry.rs` for layout): derives final per-repo hashed
//! `(bytes, files)` from the snapshot manifest the protocol just wrote.
//! Ordinary runs use [`final_per_repo_progress`] with the original
//! unbounded manifest read. A marker-broken completed run reuses its bounded
//! capture when it succeeds; if capture is refused, attribution stays at the
//! zero/unknown values set by the protocol and the manifest is not reread.
//! Small registry helpers (`unix_nanos`, `live_repo_count`, the manual
//! `full_error_chain`), the public status snapshot and the production seal
//! re-copy cap moved here for layout with every call site kept.

use std::path::PathBuf;
use std::time::SystemTime;

use shamir_db::ShamirDb;
use tokio::runtime::Handle;

use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::online::progress::{BackupProgress, BackupStage};
use crate::sync::durability::backup::online::protocol::{
    BackupReport, OnlineBackupError, SealRecopyCap,
};

/// Production seal re-copy cap: 8 s monotonic budget per repo including
/// the seal barrier, NO byte cap (R2a-4 follow-up 4; `registry` module doc).
/// The budget boundary is the absolute `Instant` handed to
/// `OnlineBackupPort::seal_point_before`, so the barrier itself counts;
/// the copy engine checks it cooperatively INSIDE its I/O paths.
/// Wall-clock caps cannot preempt in-flight syscalls (fail-closed
/// overrun documented in the `registry` module doc).
const SEAL_RECOPY_MAX_SECONDS: u64 = 8;

/// The production cap (used by the registry's `RunJob` and the cap
/// regression test): 8 s per repo, `max_bytes = None` — the seal re-copy is
/// a full-file rewrite, so no size threshold belongs here.
pub(crate) fn production_seal_cap() -> SealRecopyCap {
    SealRecopyCap {
        max_bytes: None,
        max_seconds: Some(SEAL_RECOPY_MAX_SECONDS),
    }
}

/// Immutable snapshot of the running/last backup (design §4.7 `BackupStatus`
/// payload shape; the wire mapping is a later step). Re-exported from
/// `registry` for every existing path.
#[derive(Debug, Clone)]
pub struct BackupStatusSnapshot {
    pub backup_id: String,
    /// The operator-supplied destination root.
    pub destination: PathBuf,
    /// The actual `<destination>/<timestamp>` directory, known on success
    /// (the protocol removes it on every failure path).
    pub dest_dir: Option<PathBuf>,
    pub started_at_unix_ns: u128,
    pub running: bool,
    pub cancel_requested: bool,
    /// Live stage: the adapter's port-call hints while running, the
    /// protocol's final stage once finished.
    pub stage: BackupStage,
    /// Progress counters; per-repo hashed bytes/files are updated by the
    /// protocol internally and become visible when the run finishes (the
    /// protocol exposes no progress callback — reported gap).
    pub progress: BackupProgress,
    /// Repos excluded from the captured set with reasons (Q8/Q9).
    pub excluded_repos: Vec<String>,
    /// Repos in the step-A snapshot.
    pub repos_in_snapshot: usize,
    /// Catalogue rows beyond the snapshot at run end (created during the
    /// backup, excluded per Q9).
    pub repos_created_during: usize,
    pub last_error: Option<String>,
}

/// Final per-repo hashed `(repo, bytes, files)` for a finished run,
/// derived owner-side from the snapshot manifest the protocol just wrote
/// (unbounded read — the protocol self-verified this exact file moments
/// ago): each captured repo is attributed the snapshot files under its
/// store dir and its WAL dir. Files outside every repo's subtree
/// (residual pass, system store) stay unattributed. Best-effort: on any
/// read or decode gap an empty result is returned and the repos keep the
/// zeroed entries the protocol noted in step B.
pub(super) fn final_per_repo_progress(report: &BackupReport) -> Vec<(String, u64, u64)> {
    let parsed = std::fs::read_to_string(report.dest_dir.join("manifest.json"))
        .ok()
        .and_then(|raw| manifest::decode(&raw).ok());
    from_decoded(parsed)
}

/// [`final_per_repo_progress`] over bytes from a successful bounded completion
/// capture: attribution uses those exact bytes with no second filesystem read.
/// A refused bounded capture does not call this or the unbounded fallback.
pub(super) fn final_per_repo_progress_bytes(raw: &[u8]) -> Vec<(String, u64, u64)> {
    from_decoded(
        std::str::from_utf8(raw)
            .ok()
            .and_then(|raw| manifest::decode(raw).ok()),
    )
}

fn from_decoded(parsed: Option<manifest::ManifestV2>) -> Vec<(String, u64, u64)> {
    let Some(manifest_data) = parsed else {
        return Vec::new();
    };
    let Some(consistency) = manifest_data.consistency.as_ref() else {
        return Vec::new();
    };
    // WAL destination dirs -> owning repo (the manifest's WAL section
    // already records that attribution).
    let wal_repo = |path: &str| -> Option<&str> {
        manifest_data
            .wal
            .as_ref()?
            .dirs
            .iter()
            .find(|dir| path.starts_with(&format!("{}/", dir.dir)))
            .map(|dir| dir.repo.as_str())
    };
    let mut totals: Vec<(String, u64, u64)> = consistency
        .per_repo
        .keys()
        .map(|repo| (repo.clone(), 0, 0))
        .collect();
    for file in &manifest_data.files {
        let repo = consistency
            .per_repo
            .iter()
            .find(|(_, entry)| file.path.starts_with(&format!("{}/", entry.store_dir)))
            .map(|(repo, _)| repo.as_str())
            .or_else(|| wal_repo(&file.path));
        if let Some(repo) = repo {
            if let Some(entry) = totals.iter_mut().find(|(name, _, _)| name == repo) {
                entry.1 += file.size_bytes;
                entry.2 += 1;
            }
        }
    }
    totals
}
/// Best-effort live catalogue row count; a failed read reports 0 (the
/// created-during note then reads 0 rather than failing the record).
pub(super) fn live_repo_count(db: &ShamirDb, runtime: &Handle) -> usize {
    let stores = db.system_store();
    runtime
        .block_on(Box::pin(stores.load_repositories()))
        .map(|records| records.len())
        .unwrap_or(0)
}

pub(super) fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

/// The error's `Display` chain, top error first. The protocol's top-level
/// `Display` already carries the stage (`failed at stage Sealing`); the
/// source chain contributes the typed failure detail (e.g. the
/// `RecopyCap` bytes-versus-cap text) that `to_string()` alone would drop,
/// so `BackupStatus`'s `last_error` keeps both stage and cap detail for a
/// MANUAL run.
pub(super) fn full_error_chain(error: &OnlineBackupError) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(link) = source {
        text.push_str(": ");
        text.push_str(&link.to_string());
        source = link.source();
    }
    text
}
