use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backup::{verify_manifest, Manifest, MANIFEST_FILE_NAME};
use shamir_sync::format_stamp::{decode, FormatStamp, STAMP_FILE_NAME};

/// Successful evidence from a verified, fresh, nonempty pre-migration backup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupEvidence {
    pub files_checked: u64,
}

/// Verify the backup against the live pre-migration stamp and directory names.
pub(crate) fn assess_backup(
    live_dir: &Path,
    backup_dir: &Path,
    live_stamp: Option<&FormatStamp>,
    max_age_hours: u64,
    configured_log_name: Option<&Path>,
) -> Result<BackupEvidence, String> {
    let live_stamp = live_stamp.ok_or_else(|| {
        "a stampless data directory has no copied stamp to compare; the default backup gate cannot prove it. Use --i-have-a-backup only after verifying a backup yourself".to_owned()
    })?;

    let verify = verify_manifest(backup_dir).map_err(|error| error.to_string())?;
    let manifest_path = backup_dir.join(MANIFEST_FILE_NAME);
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("decode {}: {error}", manifest_path.display()))?;

    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock precedes the Unix epoch: {error}"))?
        .as_nanos();
    if manifest.created_at_unix_ns > now_ns {
        return Err("backup manifest timestamp is in the future".to_owned());
    }
    let max_age_ns = u128::from(max_age_hours)
        .checked_mul(3_600)
        .and_then(|seconds| seconds.checked_mul(1_000_000_000))
        .ok_or_else(|| "--max-backup-age-hours overflows the freshness calculation".to_owned())?;
    let age_ns = now_ns
        .checked_sub(manifest.created_at_unix_ns)
        .ok_or_else(|| "backup manifest timestamp is in the future".to_owned())?;
    if age_ns > max_age_ns {
        return Err(format!(
            "backup exceeds the freshness window of {max_age_hours} hour(s)"
        ));
    }

    if !manifest
        .files
        .iter()
        .any(|entry| entry.path == STAMP_FILE_NAME)
    {
        return Err("backup manifest does not cover format.stamp".to_owned());
    }

    let stamp_path = backup_dir.join(STAMP_FILE_NAME);
    let copied_stamp_bytes = fs::read(&stamp_path)
        .map_err(|error| format!("backup has no readable format.stamp: {error}"))?;
    let copied_stamp = decode(&copied_stamp_bytes, backup_dir)
        .map_err(|error| format!("backup format.stamp is invalid: {error}"))?;
    if copied_stamp.format_epoch() != live_stamp.format_epoch() {
        return Err(format!(
            "backup stamp epoch {} does not match the live pre-migration epoch {}",
            copied_stamp.format_epoch(),
            live_stamp.format_epoch()
        ));
    }
    if copied_stamp.created_by() != live_stamp.created_by() {
        return Err(
            "backup stamp created_by does not match the live pre-migration stamp".to_owned(),
        );
    }

    let mut backup_top_level = BTreeSet::new();
    let mut live_has_user_data = false;
    let mut backup_has_user_data = false;
    for entry in &manifest.files {
        let normalized = entry.path.replace('\\', "/");
        if let Some(name) = normalized.split('/').next().filter(|name| !name.is_empty()) {
            backup_top_level.insert(name.to_owned());
            if is_user_data_name(name, configured_log_name) {
                backup_has_user_data = true;
            }
        }
    }

    for entry in fs::read_dir(live_dir)
        .map_err(|error| format!("read live data directory {}: {error}", live_dir.display()))?
    {
        let entry = entry.map_err(|error| format!("read live data-directory entry: {error}"))?;
        let name = entry.file_name();
        if coverage_exemption(&name) {
            continue;
        }
        let Some(name_text) = name.to_str() else {
            return Err("live data directory has a non-UTF-8 top-level name that the backup manifest cannot prove".to_owned());
        };
        if !backup_top_level.contains(name_text) {
            return Err(format!(
                "backup manifest does not cover live top-level entry {name_text:?} (empty directories may not appear in manifests)"
            ));
        }
        if is_user_data_name(name_text, configured_log_name) {
            live_has_user_data = true;
        }
    }
    if !live_has_user_data || !backup_has_user_data {
        return Err("backup/live directory contains no manifest-backed user data (stamp/lock/journal/cache/log-only snapshots are rejected)".to_owned());
    }

    Ok(BackupEvidence {
        files_checked: verify.files_checked,
    })
}

fn coverage_exemption(name: &OsStr) -> bool {
    name == OsStr::new("cache")
        || name == OsStr::new(".shamir.lock")
        || name == OsStr::new("format_migration.journal")
}

fn is_user_data_name(name: &str, configured_log_name: Option<&Path>) -> bool {
    name != "cache"
        && name != ".shamir.lock"
        && name != "format.stamp"
        && name != "format.stamp.tmp"
        && name != "format_migration.journal"
        && configured_log_name.is_none_or(|log_name| Path::new(name) != log_name)
}
