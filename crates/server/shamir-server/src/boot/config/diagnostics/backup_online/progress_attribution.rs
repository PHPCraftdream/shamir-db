//! Owner-side progress attribution for a finished online backup (moved out
//! of `registry.rs` for layout): derives final per-repo hashed
//! `(bytes, files)` from the snapshot manifest the protocol just wrote.
//! Ordinary runs use [`final_per_repo_progress`] with the original
//! unbounded manifest read; a marker-broken run reuses the ONE capped
//! completion read the registry already performed for the adoption-digest
//! capture via [`final_per_repo_progress_bytes`] — no second read.

use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::online::protocol::BackupReport;

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

/// [`final_per_repo_progress`] over manifest bytes the caller already
/// read (the registry's capped completion capture on a marker-broken
/// run): attribution is derived from those exact bytes, never a second
/// filesystem read. Same best-effort contract.
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
