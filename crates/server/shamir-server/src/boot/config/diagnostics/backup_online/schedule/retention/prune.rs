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
    // deletes; the next repaired pass does the work.
    provision_snapshot_lease(&canonical).map_err(|_| Refusal::Incomplete)?;

    let mut owned = Vec::new();
    let mut untouched = 0;
    let mut fresh = None;
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
    // Exact provenance matches: (name, manifest creation time, manifest
    // SHA-256 digest) triples the registry itself published, verified and
    // kept in THIS root under THIS owner while the lease marker was
    // broken. Nothing else untagged is ever adopted; a different output
    // under a recorded name is bound out by the creation time AND digest.
    // Adoption happens only when marker provisioning succeeded (it runs
    // BEFORE the scan): a broken marker fails closed before any scan, tag
    // or deletion. The unavoidable pre-marker transition (a reader may
    // have resolved a copy while the marker was absent) is covered only
    // by the documented stable-marker / operator-quiescence contract.
    let mut adopt: Vec<(String, u128, PathBuf, String)> = Vec::new();
    let entries = fs::read_dir(&canonical).map_err(|_| Refusal::Incomplete)?;
    let mut index = 0usize;
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
        // Crash-debris cleanup DURING the initial discovery (reviewer P2):
        // a staged sidecar left by an earlier crashed pass is removed
        // here, per entry, instead of a second full pre-scan directory
        // listing. A successfully removed entry neither consumes a
        // visit-cap slot nor counts; a failed cleanup stays a normal
        // counted entry, so cleanup never bypasses the cap nor fails the
        // pass. Cancellation is the loop's existing checkpoint.
        if remove_staged_debris_entry(&name, &path, &root_meta) {
            continue;
        }
        if index >= ROOT_ENTRIES {
            return Err(Refusal::Budget);
        }
        index += 1;
        let is_fresh = name == fresh_name;
        let name_str = name.to_str();
        let pending_records = name_str.and_then(|name| pending_by_name.get(name));
        let is_pending = pending_records.is_some();
        let mut classify = || -> Result<_, Refusal> {
            if !timestamp_name(&name) {
                return Err(Refusal::Unverifiable);
            }
            check_child(&path, &root_meta)?;
            let sidecar = read_sidecar(&path, &root_meta, budget)?;
            if (!is_fresh && !is_pending) || has_tag(&sidecar) {
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
            Ok((time, sidecar, _)) if is_fresh => {
                fresh = Some((time, path, sidecar));
            }
            // The scan-time sidecar is only a cheap gate: the decision to
            // tag is made on a FRESH sidecar read AND a FRESH manifest
            // digest immediately below, which is what detects a
            // concurrently swapped tag or a replaced manifest (unsupported
            // writers); the scan snapshot makes no such claim. A recorded
            // (name, creation time, MANIFEST DIGEST) triple is adoptable:
            // the digest binds the record to the recorded OUTPUT's exact
            // manifest bytes, so a replacement tree under the same name
            // with the same creation time (but different bytes) is adopted
            // never — but if it is already owner-tagged it stays a normal
            // selection candidate exactly as before (selection unchanged),
            // otherwise untouched. The `untouched` metric is published only
            // at the complete-scan point below.
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
    let (time, fresh_path, sidecar) = fresh.ok_or(Refusal::Unverifiable)?;
    check_root(root, &canonical)?;
    check_child(&fresh_path, &root_meta)?;
    tag_fresh(&canonical, &fresh_path, sidecar, owner, budget)?;

    #[cfg(test)]
    tracing::trace!(
        target: "shamir_server::retention_before_adoption",
        "retention scan verified candidates"
    );

    // Marker is fresh and verified: adopt exactly the provenance matches.
    // Every child is revalidated immediately before tagging — same root,
    // same plain-directory disposition the scan enforced — and BOTH the
    // sidecar and the manifest digest are re-checked FRESH, so a tag or
    // manifest swapped since the scan is honored, not missed. Nothing is
    // deleted before this loop.
    for (name, time, path, digest) in adopt {
        budget.checkpoint()?;
        check_root(root, &canonical)?;
        check_child(&path, &root_meta)?;
        let sidecar = read_sidecar(&path, &root_meta, budget)?;
        // Bounded re-read of the exact manifest bytes: same cap as the
        // scan (MAX_MANIFEST) and the same Budget charging; a normal read
        // refusal fails closed (untouched), never weakens the refusal.
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
        // The scan enqueued this entry only on an exact (name, time,
        // digest) record match, and the re-read digest above equals the
        // scanned one — nothing else to check before tagging.
        if digest_now != digest {
            untouched += 1;
            outcome.untouched = Some(untouched);
            continue;
        }
        if has_tag(&sidecar) {
            if is_owned(&sidecar, owner) {
                // Already tagged by an earlier repaired pass: nothing left
                // to adopt, but it is a normal selection candidate.
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

    owned.push((time, fresh_path.clone()));
    owned.sort_unstable(); // Creation time, then direct-child name.
                           // A hard newest-N bound already selects every age-eligible copy outside
                           // the protected set. max_age_days cannot reduce the newest-N floor.
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
