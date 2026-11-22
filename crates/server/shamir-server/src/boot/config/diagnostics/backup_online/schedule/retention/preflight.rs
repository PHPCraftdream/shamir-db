use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::path::Path;

use crate::sync::durability::backup::online::manifest::{
    self, BackupKind, BACKUP_META_FILE_NAME, MANIFEST_FORMAT_VERSION_V2,
};
use crate::sync::durability::backup::{
    verify_retention_candidate, BackupError, MANIFEST_FILE_NAME,
};
use crate::sync::format_stamp::STAMP_FILE_NAME;

use super::super::retention_pass::{manifest_digest_hex, MAX_MANIFEST};
use super::budget::{metadata, plain_on_root, Budget, Refusal};

const MAX_DEPTH: usize = 32;
// A stamp frame has a 64 KiB maximum body plus header and digest.
const MAX_STAMP: u64 = (64 << 10) + 50;

/// Verified manifest creation time at `dir`, plus — only when
/// `want_digest` is set — the SHA-256 (lowercase hex, via the retention-pass
/// `manifest_digest_hex` helper) of the raw manifest bytes this verification
/// read. Adoption compares the captured creation time and manifest digest
/// against the scan and late reread; this does not bind payload bytes or
/// directory identity, which rely on the single-writer-root contract.
/// Digest computation is skipped for names without a pending record, so a
/// normal pass pays no hashing overhead.
/// Match exact paths from the actual directory listing before allowing the
/// verifier to resolve names. This also excludes filesystem aliases that
/// aren't listed (e.g. Windows short names or case aliases).
pub(super) fn verified_time(
    dir: &Path,
    root_meta: &Metadata,
    budget: &mut Budget<'_>,
    want_digest: bool,
) -> Result<(u128, Option<String>), Refusal> {
    let raw = budget.read(&dir.join(MANIFEST_FILE_NAME), root_meta, MAX_MANIFEST)?;
    let text = std::str::from_utf8(&raw).map_err(|_| Refusal::Unverifiable)?;
    let decoded = manifest::decode(text).map_err(|_| Refusal::Unverifiable)?;
    if decoded.format_version != MANIFEST_FORMAT_VERSION_V2
        || !matches!(decoded.kind, BackupKind::Online)
    {
        return Err(Refusal::Unverifiable);
    }
    let wal = decoded.wal.as_ref().ok_or(Refusal::Unverifiable)?;
    let points = decoded.consistency.as_ref().ok_or(Refusal::Unverifiable)?;
    if decoded.files.len() > 100_000 || wal.dirs.len() > 100_000 || points.per_repo.len() > 100_000
    {
        return Err(Refusal::Budget);
    }

    let mut tree = BTreeMap::new();
    walk(dir, "", 0, root_meta, budget, &mut tree)?;
    file_size(&tree, MANIFEST_FILE_NAME)?;
    file_size(&tree, BACKUP_META_FILE_NAME)?;
    // Account for verifier reads in addition to our own reads.
    budget.bytes(raw.len() as u64)?;
    match metadata(&dir.join(STAMP_FILE_NAME)) {
        Ok(_) => {
            let size = file_size(&tree, STAMP_FILE_NAME)?;
            if size > MAX_STAMP {
                return Err(Refusal::Budget);
            }
            budget.bytes(size)?;
        }
        Err(Refusal::Unverifiable) => {} // Absent stamp is permitted by v2.
        Err(error) => return Err(error),
    }
    let mut claimed = BTreeSet::new();
    for entry in &decoded.files {
        budget.checkpoint()?;
        if !claimed.insert(entry.path.as_str()) {
            return Err(Refusal::Unverifiable);
        }
        budget.bytes(file_size(&tree, &entry.path)?)?;
    }
    let mut wal_dirs = BTreeSet::new();
    let mut wal_files = BTreeSet::new();
    for listing in &wal.dirs {
        budget.checkpoint()?;
        if tree.get(&listing.dir) != Some(&None) || !wal_dirs.insert(listing.dir.as_str()) {
            return Err(Refusal::Unverifiable);
        }
        for segment in &listing.segments {
            budget.checkpoint()?;
            if !claimed.contains(segment.file.as_str()) || !wal_files.insert(segment.file.as_str())
            {
                return Err(Refusal::Unverifiable);
            }
            budget.bytes(file_size(&tree, &segment.file)?)?;
            if let Some(sidecar) = &segment.sidecar {
                if !claimed.contains(sidecar.as_str()) {
                    return Err(Refusal::Unverifiable);
                }
            }
        }
    }

    // The verifier ignores empty directories. The producer can also leave
    // empty parents if a source disappears after mkdir. Without provenance
    // for empty directories, preserve the whole candidate in either case.
    let mut known_dirs = BTreeSet::new();
    for mut path in claimed.iter().copied() {
        while let Some((parent, _)) = path.rsplit_once('/') {
            if !known_dirs.insert(parent) {
                break;
            }
            path = parent;
        }
    }
    for (path, size) in &tree {
        budget.checkpoint()?;
        let known = if size.is_some() {
            path == MANIFEST_FILE_NAME
                || path == BACKUP_META_FILE_NAME
                || claimed.contains(path.as_str())
        } else {
            known_dirs.contains(path.as_str())
        };
        if !known {
            return Err(Refusal::Unverifiable);
        }
    }

    budget.checkpoint()?;
    // The verifier checks the bounded manifest bytes `raw` without a
    // second manifest read. This preserves manifest consistency for this
    // check, but does not prove directory identity or prevent later payload
    // changes under the single-writer-root contract.
    let verified = verify_retention_candidate(dir, &raw);
    budget.checkpoint()?;
    verified.map_err(|error| match error {
        BackupError::Io(_) => Refusal::Incomplete,
        _ => Refusal::Unverifiable,
    })?;
    // Digest the same manifest bytes checked above; this binds the manifest
    // contents only, not the payload tree or directory identity.
    let digest = want_digest.then(|| manifest_digest_hex(&raw));
    Ok((decoded.created_at_unix_ns, digest))
}

fn file_size(tree: &BTreeMap<String, Option<u64>>, path: &str) -> Result<u64, Refusal> {
    tree.get(path)
        .copied()
        .flatten()
        .ok_or(Refusal::Unverifiable)
}

fn walk(
    dir: &Path,
    prefix: &str,
    depth: usize,
    root_meta: &Metadata,
    budget: &mut Budget<'_>,
    tree: &mut BTreeMap<String, Option<u64>>,
) -> Result<(), Refusal> {
    budget.checkpoint()?;
    let entries = fs::read_dir(dir).map_err(|_| Refusal::Incomplete)?;
    for entry in entries {
        budget.entry(tree.len())?;
        if depth >= MAX_DEPTH {
            return Err(Refusal::Budget);
        }
        let entry = entry.map_err(|_| Refusal::Incomplete)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(Refusal::Unverifiable)?;
        if name.contains(['\\', ':']) || name.ends_with(['.', ' ']) {
            return Err(Refusal::Unverifiable);
        }
        let path = entry.path();
        let meta = metadata(&path).map_err(|_| Refusal::Incomplete)?;
        if !plain_on_root(&meta, root_meta) {
            return Err(Refusal::Unverifiable);
        }
        let relative = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if meta.is_dir() {
            tree.insert(relative.clone(), None);
            walk(&path, &relative, depth + 1, root_meta, budget, tree)?;
        } else {
            // Charge every regular file visited, including unknown files in
            // copies that will be rejected. Verifier reads are charged again.
            budget.bytes(meta.len())?;
            tree.insert(relative, Some(meta.len()));
        }
    }
    Ok(())
}
