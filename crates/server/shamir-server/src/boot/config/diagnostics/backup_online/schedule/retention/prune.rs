use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use shamir_private_dir::verify_private_dir;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::{
    provision_snapshot_lease, try_snapshot_delete_lease, MANIFEST_FILE_NAME,
    SNAPSHOT_LEASE_FILE_NAME,
};

use super::super::retention_pass::{manifest_digest_hex, MAX_MANIFEST};
use super::budget::{metadata, plain_on_root, Budget, Refusal, ROOT_ENTRIES};
use super::ownership::{has_tag, is_owned, read_sidecar, remove_staged_debris_entry, tag_fresh};
use super::preflight::verified_time;
use super::RetentionOutcome;

/// Run only after protocol success, holding the existing registry admission
/// until return. The private root must be the canonical
/// path retained at boot, and the tree must have no concurrent writers.
/// Same-account operators may copy out, but must not modify or mount
/// anything inside this tree.
pub(in crate::boot::config::diagnostics::backup_online) fn prune(
    root: &Path,
    fresh_copy: &Path,
    owner: &str,
    policy: &BackupRetentionConfig,
    cancel: &CancelFlag,
    pending: &BTreeSet<(String, u128, String)>,
) -> RetentionOutcome {
    let mut outcome = RetentionOutcome::default();
    let mut budget = Budget::new(cancel);
    if let Err(error) = prune_inner(
        root,
        fresh_copy,
        owner,
        policy,
        pending,
        &mut budget,
        &mut outcome,
    ) {
        if !matches!(error, Refusal::Cancelled) {
            outcome.failed += 1;
            tracing::warn!(reason = ?error, "scheduled backup retention stopped");
        }
    }
    outcome
}

pub(super) fn prune_inner(
    root: &Path,
    fresh_copy: &Path,
    owner: &str,
    policy: &BackupRetentionConfig,
    pending: &BTreeSet<(String, u128, String)>,
    budget: &mut Budget<'_>,
    outcome: &mut RetentionOutcome,
) -> Result<(), Refusal> {
    budget.checkpoint()?;
    if owner.is_empty() || owner.len() > 256 || policy.keep_last == 0 || !root.is_absolute() {
        return Err(Refusal::Incomplete);
    }
    verify_private_dir(root).map_err(|_| Refusal::Incomplete)?;
    let canonical = fs::canonicalize(root).map_err(|_| Refusal::Incomplete)?;
    if canonical != root || fresh_copy.parent() != Some(root) {
        return Err(Refusal::Unverifiable);
    }
    let root_meta = metadata(&canonical)?;
    let fresh_name = fresh_copy.file_name().ok_or(Refusal::Unverifiable)?;
    if !timestamp_name(fresh_name) {
        return Err(Refusal::Unverifiable);
    }
    // Provision the cross-process lease marker BEFORE the scan (after the
    // root/fresh checks): readers NEVER create it — a root without it has
    // no managed reads to protect, and this pass must gain it here. Failing
    // before the scan keeps `untouched` unaccounted (an incomplete scan
    // leaves the last complete classification intact) and never tags or
    // deletes; a later repaired pass may retry, but no work is promised.
    provision_snapshot_lease(&canonical).map_err(|_| Refusal::Incomplete)?;

    let mut owned = Vec::new();
    let mut untouched = 0;
    // O(log n) provenance lookups: (name -> set of recorded (manifest
    // creation time, manifest SHA-256 digest)), built once for the whole
    // scan.
    let mut pending_by_name: BTreeMap<String, BTreeSet<(u128, String)>> = BTreeMap::new();
    for (name, time, digest) in pending {
        pending_by_name
            .entry(name.clone())
            .or_default()
            .insert((*time, digest.clone()));
    }
    // Provenance matches compare a recorded name, manifest creation time and
    // digest. They bind manifest bytes at post-protocol capture, scan and
    // late reread, not payload bytes or directory identity. Capture is after
    // protocol return, so a replacement already present then is what gets
    // recorded. Adoption assumes one writer owns the root; sharing is not
    // enforced here. Untagged candidates are tagged only after the late
    // sidecar check; already-owned matching entries need no new tag.
    let mut adopt: Vec<(String, u128, PathBuf, String)> = Vec::new();

    // Verify and tag the fresh copy BEFORE the root is scanned: a verified
    // output of this server stays owned even when the scan then refuses
    // (budget, cancellation, an unreadable foreign entry) or the process dies
    // right after, so a later pass can select it instead of skipping it as
    // foreign. The tag still follows the child check and the late root and
    // child re-checks, and an unverifiable fresh copy is never tagged.
    // Nothing is deleted here.
    let fresh_path = fresh_copy.to_path_buf();
    let fresh_is_pending = fresh_name
        .to_str()
        .is_some_and(|name| pending_by_name.contains_key(name));
    check_child(&fresh_path, &root_meta)?;
    let fresh_sidecar = read_sidecar(&fresh_path, &root_meta, budget)?;
    if has_tag(&fresh_sidecar) && !is_owned(&fresh_sidecar, owner) {
        return Err(Refusal::Unverifiable);
    }
    let (fresh_time, _) = verified_time(&fresh_path, &root_meta, budget, fresh_is_pending)?;
    check_root(root, &canonical)?;
    check_child(&fresh_path, &root_meta)?;
    tag_fresh(&canonical, &fresh_path, fresh_sidecar, owner, budget)?;

    // The fresh copy already took its root entry slot, so the scan starts at
    // one and skips that name.
    let mut index = 1usize;
    let entries = fs::read_dir(&canonical).map_err(|_| Refusal::Incomplete)?;
    for entry in entries {
        budget.checkpoint()?;
        let entry = entry.map_err(|_| Refusal::Incomplete)?;
        let name = entry.file_name();
        let path = entry.path();
        if path.parent() != Some(canonical.as_path()) {
            return Err(Refusal::Incomplete);
        }
        // The reserved lease marker is never a candidate: skip it without
        // advancing the non-marker index, so it neither consumes a visit-cap
        // slot nor counts as untouched. Selection, checks and budget change
        // not at all for timestamped entries.
        if name == SNAPSHOT_LEASE_FILE_NAME {
            continue;
        }
        // Verified and tagged above; the scan never reclassifies it.
        if name == fresh_name {
            continue;
        }
        // Crash-debris cleanup DURING the initial discovery: only a direct,
        // plain regular file with the reserved exact name shape and age
        // threshold is removed before the cap. The age rule protects another
        // runner's live staging file only while its reported modification time
        // is younger than the threshold; it is not a lock, so one runner must
        // own the root by contract (sharing is not enforced by this code).
        if remove_staged_debris_entry(&name, &path, &root_meta) {
            continue;
        }
        if index >= ROOT_ENTRIES {
            return Err(Refusal::Budget);
        }
        index += 1;
        let name_str = name.to_str();
        let pending_records = name_str.and_then(|name| pending_by_name.get(name));
        let is_pending = pending_records.is_some();
        let mut classify = || -> Result<_, Refusal> {
            if !timestamp_name(&name) {
                return Err(Refusal::Unverifiable);
            }
            check_child(&path, &root_meta)?;
            let sidecar = read_sidecar(&path, &root_meta, budget)?;
            if !is_pending || has_tag(&sidecar) {
                // A copy already tagged by anyone must be owned by us to
                // stay a selection candidate; an untagged copy is eligible
                // only as the fresh output or as an exact provenance match.
                if !is_owned(&sidecar, owner) {
                    return Err(Refusal::Unverifiable);
                }
            }
            // Digest only for pending names: normal passes pay no SHA.
            let (time, digest) = verified_time(&path, &root_meta, budget, is_pending)?;
            Ok((time, sidecar, digest))
        };
        match classify() {
            // A recorded name/time/digest match means only that the manifest
            // bytes captured after protocol success still match at scan and
            // late check. It does not bind payload bytes or directory
            // identity; unchanged-manifest replacements are outside the
            // single-writer-root contract. An untagged copy can be tagged
            // only while both sidecar checks find no tag; a matching entry
            // already bearing this owner's tag remains a normal selection
            // candidate and may be reported without a new tag.
            Ok((time, sidecar, digest)) if is_pending => {
                let matched = pending_records.is_some_and(|records| {
                    digest
                        .as_ref()
                        .is_some_and(|digest| records.contains(&(time, digest.clone())))
                });
                if matched {
                    // A pending name always verifies with a digest here.
                    let name = name_str.expect("pending name is valid Unicode");
                    let digest = digest.expect("pending manifest digest computed");
                    adopt.push((name.to_owned(), time, path, digest));
                } else if is_owned(&sidecar, owner) {
                    owned.push((time, path));
                } else {
                    untouched += 1;
                }
            }
            Ok((time, _, _)) => owned.push((time, path)),
            Err(Refusal::Unverifiable) => untouched += 1,
            Err(error) => return Err(error),
        }
    }
    budget.checkpoint()?;
    outcome.untouched = Some(untouched);

    // Fires after the fresh copy was tagged and the root was scanned, still
    // before any adoption tag or deletion; the token is consumed only by the
    // retention tests.
    #[cfg(test)]
    tracing::trace!(
        target: "shamir_server::retention_before_adoption",
        "retention scan verified candidates"
    );

    // Marker is available and the fresh copy was checked: adopt only the
    // recorded provenance matches. Before tagging each untagged candidate,
    // re-check the root/child disposition, sidecar and bounded manifest
    // bytes. This catches sidecar or manifest changes since scan, but does
    // not reverify payloads or bind directory identity; those rely on the
    // single-writer-root contract. Nothing is deleted before this loop.
    for (name, time, path, digest) in adopt {
        budget.checkpoint()?;
        check_root(root, &canonical)?;
        check_child(&path, &root_meta)?;
        let sidecar = read_sidecar(&path, &root_meta, budget)?;
        // Bounded late re-read of the manifest: same cap as the scan
        // (MAX_MANIFEST) and the same Budget charging. A failed read leaves
        // this entry untagged; it does not turn the refusal into adoption.
        let raw = match budget.read(&path.join(MANIFEST_FILE_NAME), &root_meta, MAX_MANIFEST) {
            Ok(raw) => raw,
            Err(Refusal::Unverifiable) => {
                untouched += 1;
                outcome.untouched = Some(untouched);
                continue;
            }
            Err(error) => return Err(error),
        };
        let digest_now = manifest_digest_hex(&raw);
        // Late bounded manifest reread: a refusal keeps this candidate
        // untagged. The digest covers manifest bytes only; payload bytes and
        // directory identity rely on the single-writer root contract.
        if digest_now != digest {
            untouched += 1;
            outcome.untouched = Some(untouched);
            continue;
        }
        if has_tag(&sidecar) {
            if is_owned(&sidecar, owner) {
                // The matching entry already has our tag; do not write a
                // second tag, but report this provenance match as retired.
                outcome.adopted.push((name, time, digest_now));
                owned.push((time, path));
            } else {
                untouched += 1;
                outcome.untouched = Some(untouched);
            }
            continue;
        }
        match tag_fresh(&canonical, &path, sidecar, owner, budget) {
            Ok(()) => {
                outcome.adopted.push((name, time, digest_now));
                owned.push((time, path));
            }
            Err(Refusal::Unverifiable) => {
                untouched += 1;
                outcome.untouched = Some(untouched);
            }
            Err(error) => return Err(error),
        }
    }

    owned.push((fresh_time, fresh_path.clone()));
    owned.sort_unstable(); // Creation time, then direct-child name.
                           // A hard newest-N bound already selects every copy outside the
                           // protected set; the just-created copy stays protected separately.
    let excess = owned.len().saturating_sub(policy.keep_last);
    for (_, path) in owned.into_iter().take(excess) {
        budget.checkpoint()?;
        if path == fresh_path {
            continue;
        }
        check_root(root, &canonical)?;
        check_child(&path, &root_meta)?;
        budget.checkpoint()?;
        // Fail closed on lease errors (account as failure, no delete);
        // `None` is ordinary contention with a reader: skip, not an error.
        let lease = match try_snapshot_delete_lease(&canonical) {
            Ok(Some(lease)) => lease,
            Ok(None) => {
                outcome.skipped_in_use += 1;
                continue;
            }
            Err(error) => {
                outcome.failed += 1;
                tracing::warn!(
                    kind = ?error.kind(),
                    "scheduled backup retention could not take the delete lease"
                );
                continue;
            }
        };
        // Hold the exclusive lease through the deletion so no reader can
        // start between the last check and the recursive remove.
        if let Err(error) = fs::remove_dir_all(&path) {
            outcome.failed += 1;
            tracing::warn!(kind = ?error.kind(), "scheduled backup retention deletion failed");
        } else {
            outcome.deleted += 1;
        }
        drop(lease);
    }
    Ok(())
}

fn check_root(root: &Path, canonical: &Path) -> Result<(), Refusal> {
    verify_private_dir(root).map_err(|_| Refusal::Incomplete)?;
    if fs::canonicalize(root).map_err(|_| Refusal::Incomplete)? != canonical {
        return Err(Refusal::Incomplete);
    }
    Ok(())
}

fn check_child(path: &Path, root_meta: &Metadata) -> Result<(), Refusal> {
    let meta = metadata(path)?;
    if !meta.is_dir() || !plain_on_root(&meta, root_meta) {
        return Err(Refusal::Unverifiable);
    }
    Ok(())
}

fn timestamp_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let bytes = name.as_bytes();
    if bytes.len() != 15
        || bytes[8] != b'_'
        || !bytes[..8].iter().chain(&bytes[9..]).all(u8::is_ascii_digit)
    {
        return false;
    }
    let number = |start: usize, end: usize| {
        bytes[start..end]
            .iter()
            .fold(0u32, |value, &digit| value * 10 + u32::from(digit - b'0'))
    };
    let year = number(0, 4);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match number(4, 6) {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&number(6, 8))
        && number(9, 11) < 24
        && number(11, 13) < 60
        && number(13, 15) < 60
}
