use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::path::Path;

use crate::sync::durability::backup::online::manifest::{
    self, BackupKind, BACKUP_META_FILE_NAME, MANIFEST_FORMAT_VERSION_V2,
};
use crate::sync::durability::backup::{
    verify_retention_candidate, BackupError, MANIFEST_FILE_NAME,
};
use crate::sync::format_stamp::{inspect, StampState, STAMP_FILE_NAME};

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
    // The manifest is charged once by `budget.read` here; the verifier is
    // handed these bytes and never re-reads the manifest, so no second charge.
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

    // Discovery/validation phase: only the bounded manifest read above and
    // the walk's entry charges have been charged, so every cheap reject here
    // costs no byte budget for files the verifier would never open.
    let mut tree = BTreeMap::new();
    walk(dir, "", 0, root_meta, budget, &mut tree)?;
    file_size(&tree, MANIFEST_FILE_NAME)?;
    file_size(&tree, BACKUP_META_FILE_NAME)?;
    match metadata(&dir.join(STAMP_FILE_NAME)) {
        Ok(_) => {
            // The verifier reads the stamp twice: `format_stamp::inspect`
            // (charged here, with the verdict) and the manifest-file loop's
            // hash (charged with the claimed files below).
            let size = file_size(&tree, STAMP_FILE_NAME)?;
            if size > MAX_STAMP {
                return Err(Refusal::Budget);
            }
            // Decide the stamp verdict BEFORE the payload is charged: the
            // verifier inspects the same bytes (`check_stamp_agreement`) and
            // refuses a corrupt or newer-layout stamp, so charging the whole
            // candidate first made a large WAL pay for a copy that could never
            // verify. This is the inspect read, and it is charged exactly once.
            budget.bytes(size)?;
            match inspect(dir) {
                StampState::Valid(stamp) => match decoded.format_epoch {
                    Some(epoch) if epoch == stamp.format_epoch() => {}
                    _ => return Err(Refusal::Unverifiable),
                },
                // Absent is permitted by v2 regardless of the manifest epoch.
                StampState::Absent { .. } => {}
                StampState::Corrupt { .. } | StampState::NewerLayout { .. } => {
                    return Err(Refusal::Unverifiable);
                }
            }
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
        // Root control files are never listed in `files`: the online writer's
        // copy pass skips `manifest.json` and `backup.meta.json` at the top
        // level, so a manifest that lists either is not a copy this server
        // produced. Excluding such an entry from the hash set instead would
        // change `verify_retention_candidate`'s contract — that verifier lives
        // in `shamir-sync`'s shared backup module, hashes every listed path
        // for other callers too, and is out of this change's area — while
        // hashing bytes retention itself rewrites (`tag_fresh` replaces
        // `backup.meta.json`, and `manifest.json` is the file being verified)
        // cannot be a useful check. The copied `format.stamp` is NOT such a
        // control file: the writer does list it, so it stays hashable.
        if entry.path == BACKUP_META_FILE_NAME || entry.path == MANIFEST_FILE_NAME {
            return Err(Refusal::Unverifiable);
        }
        // The verifier refuses a manifest that lists a `cache` payload
        // (`offline.rs::is_cache_manifest_path`). Decide it HERE, before any
        // byte of the candidate is charged: a listed cache entry made a
        // candidate pay its whole payload for a read the verifier never does.
        let first_component = entry
            .path
            .replace('\\', "/")
            .split('/')
            .find(|component| !component.is_empty() && *component != ".")
            .map(str::to_owned);
        if first_component.as_deref() == Some("cache") {
            return Err(Refusal::Unverifiable);
        }
        file_size(&tree, &entry.path)?;
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
            file_size(&tree, &segment.file)?;
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

    // Byte charges come only now, after every cheap reject above passed, and
    // they mirror the verifier's real reads: the stamp is read once by
    // `format_stamp::inspect` (charged here) and hashed again by its
    // manifest-file loop (charged with the claimed files); every
    // manifest-listed file is hashed once by that loop; a WAL segment is
    // hashed a second time by the verifier's WAL loop. The walk charges no
    // bytes and the manifest bytes are handed to the verifier, never re-read.
    // The stamp's inspect read was charged in the discovery phase above; the
    // verifier's SECOND read of it is its manifest-file hash, charged with the
    // claimed files below like every other listed file.
    for entry in &decoded.files {
        budget.checkpoint()?;
        budget.bytes(file_size(&tree, &entry.path)?)?;
    }
    for listing in &wal.dirs {
        for segment in &listing.segments {
            budget.checkpoint()?;
            budget.bytes(file_size(&tree, &segment.file)?)?;
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
            // Directory entry only: bytes are charged in `verified_time`, for
            // the reads the verifier really performs.
            tree.insert(relative, Some(meta.len()));
        }
    }
    Ok(())
}
