//! Owner-side progress attribution for a finished online backup (moved out
//! of `registry.rs` for layout): derives final per-repo hashed
//! `(bytes, files)` from the snapshot manifest the protocol just wrote.

use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::online::protocol::BackupReport;

/// Final per-repo hashed `(repo, bytes, files)` for a finished run,
/// derived owner-side from the snapshot manifest the protocol just wrote:
/// each captured repo is attributed the snapshot files under its store
/// dir and its WAL dir. Files outside every repo's subtree (residual
/// pass, system store) stay unattributed. Best-effort: on any read or
/// decode gap an empty result is returned and the repos keep the zeroed
/// entries the protocol noted in step B.
pub(super) fn final_per_repo_progress(report: &BackupReport) -> Vec<(String, u64, u64)> {
    let parsed = std::fs::read_to_string(report.dest_dir.join("manifest.json"))
        .ok()
        .and_then(|raw| manifest::decode(&raw).ok());
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
