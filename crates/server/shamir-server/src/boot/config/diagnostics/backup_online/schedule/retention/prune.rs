use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::path::Path;

use shamir_private_dir::verify_private_dir;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::{
    provision_snapshot_lease, try_snapshot_delete_lease, SNAPSHOT_LEASE_FILE_NAME,
};

use super::budget::{metadata, plain_on_root, Budget, Refusal, ROOT_ENTRIES};
use super::ownership::{has_tag, is_owned, read_sidecar, tag_fresh};
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
) -> RetentionOutcome {
    let mut outcome = RetentionOutcome::default();
    let mut budget = Budget::new(cancel);
    if let Err(error) = prune_inner(root, fresh_copy, owner, policy, &mut budget, &mut outcome) {
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

    let mut owned = Vec::new();
    let mut untouched = 0;
    let mut fresh = None;
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
        if index >= ROOT_ENTRIES {
            return Err(Refusal::Budget);
        }
        index += 1;
        let is_fresh = name == fresh_name;
        let mut classify = || -> Result<_, Refusal> {
            if !timestamp_name(&name) {
                return Err(Refusal::Unverifiable);
            }
            check_child(&path, &root_meta)?;
            let sidecar = read_sidecar(&path, &root_meta, budget)?;
            if (!is_fresh || has_tag(&sidecar)) && !is_owned(&sidecar, owner) {
                return Err(Refusal::Unverifiable);
            }
            let time = verified_time(&path, &root_meta, budget)?;
            Ok((time, sidecar))
        };
        match classify() {
            Ok((time, sidecar)) if is_fresh => {
                fresh = Some((time, path, sidecar));
            }
            Ok((time, _)) => owned.push((time, path)),
            Err(Refusal::Unverifiable) => untouched += 1,
            Err(error) => return Err(error),
        }
    }
    budget.checkpoint()?;
    outcome.untouched = Some(untouched);
    let (time, fresh_path, sidecar) = fresh.ok_or(Refusal::Unverifiable)?;
    check_root(root, &canonical)?;
    check_child(&fresh_path, &root_meta)?;
    // Publish the cross-process lease marker before tagging or deleting:
    // readers bootstrap their shared lease on it (provisioning it lazily
    // when absent), so a root without it must gain it here.
    provision_snapshot_lease(&canonical).map_err(|_| Refusal::Incomplete)?;
    tag_fresh(&fresh_path, sidecar, owner, budget)?;

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
