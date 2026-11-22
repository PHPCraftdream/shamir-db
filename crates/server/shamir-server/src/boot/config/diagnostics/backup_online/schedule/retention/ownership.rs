use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use tempfile::Builder;

use crate::sync::durability::backup::online::manifest::BACKUP_META_FILE_NAME;

use super::budget::{plain_on_root, Budget, Refusal};

const TAG: &str = "scheduled_retention";
const MAX_SIDECAR: u64 = 64 << 10;

/// Recognizable fixed prefix of the staged sidecar file. The suffix is
/// exactly 16 random alphanumeric characters; cleanup removes only old,
/// plain regular files of that exact shape. The 15-minute minimum age keeps
/// a live staging file from another runner safe, but does not serialize
/// runners: one runner must own the root by contract.
pub(super) const STAGED_PREFIX: &str = ".shamir-retention-staged-";
const STAGED_RANDOM_LEN: usize = 16;
const STAGED_DEBRIS_MIN_AGE: Duration = Duration::from_secs(15 * 60);

fn has_staged_name_shape(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    name.strip_prefix(STAGED_PREFIX).is_some_and(|suffix| {
        suffix.len() == STAGED_RANDOM_LEN && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

/// Best-effort removal of ONE leftover staged sidecar debris entry: a
/// direct root entry with the exact staged name shape, at least
/// [`STAGED_DEBRIS_MIN_AGE`] old, and a plain regular file. Nonfollowing
/// metadata plus the same reparse/mount exclusion as the scan means links,
/// reparse points and directories are never removed. Missing, unreadable or
/// future modification times are not debris. Returns true only when the
/// entry was actually removed; every failure leaves it as a normal counted
/// root entry, and cleanup never fails a pass.
pub(super) fn remove_staged_debris_entry(name: &OsStr, path: &Path, root_meta: &Metadata) -> bool {
    if !has_staged_name_shape(name) {
        return false;
    }
    // Nonfollowing: a symlink (or reparse point) with the matching name is
    // NOT ours to remove.
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_file() || !plain_on_root(&meta, root_meta) {
        return false;
    }
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return false;
    };
    if age < STAGED_DEBRIS_MIN_AGE {
        return false;
    }
    match fs::remove_file(path) {
        Ok(()) => true,
        Err(error) => {
            tracing::debug!(
                kind = ?error.kind(),
                "leftover staged sidecar could not be removed; it stays a normal counted entry"
            );
            false
        }
    }
}

pub(super) fn read_sidecar(
    dir: &Path,
    root_meta: &Metadata,
    budget: &mut Budget<'_>,
) -> Result<Map<String, Value>, Refusal> {
    let bytes = budget.read(&dir.join(BACKUP_META_FILE_NAME), root_meta, MAX_SIDECAR)?;
    serde_json::from_slice(&bytes).map_err(|_| Refusal::Unverifiable)
}

pub(super) fn has_tag(sidecar: &Map<String, Value>) -> bool {
    sidecar.contains_key(TAG)
}

pub(super) fn is_owned(sidecar: &Map<String, Value>, owner: &str) -> bool {
    sidecar.get(TAG).is_some_and(|tag| {
        tag.get("version").and_then(Value::as_u64) == Some(1)
            && tag.get("owner").and_then(Value::as_str) == Some(owner)
    })
}

pub(super) fn tag_fresh(
    root: &Path,
    dir: &Path,
    sidecar: Map<String, Value>,
    owner: &str,
    budget: &Budget<'_>,
) -> Result<(), Refusal> {
    tag_fresh_impl(root, dir, sidecar, owner, budget, &mut |_| {})
}

/// Test-only seam ([`tag_fresh`]): `on_staged` fires synchronously on the
/// tagging thread AFTER the staged sidecar file exists and is flushed,
/// BEFORE the atomic persist. The staging file lives in the ROOT, never
/// inside `dir`, so a verifier holding the shared lease on `dir` across
/// this callback observes a complete snapshot without the staged file.
#[cfg(test)]
pub(super) fn tag_fresh_with_seam(
    root: &Path,
    dir: &Path,
    sidecar: Map<String, Value>,
    owner: &str,
    budget: &Budget<'_>,
    on_staged: &mut dyn FnMut(&Path),
) -> Result<(), Refusal> {
    tag_fresh_impl(root, dir, sidecar, owner, budget, on_staged)
}

fn tag_fresh_impl(
    root: &Path,
    dir: &Path,
    mut sidecar: Map<String, Value>,
    owner: &str,
    budget: &Budget<'_>,
    on_staged: &mut dyn FnMut(&Path),
) -> Result<(), Refusal> {
    if has_tag(&sidecar) {
        return if is_owned(&sidecar, owner) {
            Ok(())
        } else {
            Err(Refusal::Unverifiable)
        };
    }
    sidecar.insert(TAG.to_owned(), json!({ "version": 1, "owner": owner }));
    let mut bytes = serde_json::to_vec(&sidecar).map_err(|_| Refusal::Incomplete)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_SIDECAR {
        return Err(Refusal::Budget);
    }
    budget.checkpoint()?;
    // Stage OUTSIDE the snapshot (root sibling, same filesystem: `dir` is
    // a direct child of `root`), so the staged name can never appear in a
    // leased verifier's final walk of `dir` (finding: the verifier exempted
    // only the final sidecar and reported a staged file as UnmanifestedFile).
    // The persist is an atomic rename onto the final sidecar within that
    // same volume; a reader holding the shared lease observes either the
    // old or the new sidecar, never a partially written one, and ordinary
    // reader contention cannot strand a permanently untagged copy because
    // tagging takes no lease at all.
    let mut staged = Builder::new()
        .prefix(STAGED_PREFIX)
        .rand_bytes(STAGED_RANDOM_LEN)
        .tempfile_in(root)
        .map_err(|_| Refusal::Incomplete)?;
    staged.write_all(&bytes).map_err(|_| Refusal::Incomplete)?;
    staged
        .as_file()
        .sync_all()
        .map_err(|_| Refusal::Incomplete)?;
    on_staged(staged.path());
    budget.checkpoint()?;
    staged
        .persist(dir.join(BACKUP_META_FILE_NAME))
        .map_err(|_| Refusal::Incomplete)?;
    sync_directory(dir).map_err(|_| Refusal::Incomplete)
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> io::Result<()> {
    // The staged file was flushed before atomic replacement. Windows does
    // not support the Unix directory fsync pattern.
    Ok(())
}
