//! Bounded WAL staging for `replay_to` (R2a step 3b-1): validate the staged
//! WAL against the recorded point `V*` BEFORE anything is applied.
//!
//! Reads every segment strictly (torn tails and undecodable frames refuse),
//! checks versions against `V*` and validates the interner scope marker.
//! This phase only reads; it never opens an append handle.

use std::path::Path;

use shamir_storage::error::DbError;
use shamir_wal::{SegmentSet, WalEntryV2, WalStrictReadError};

use crate::tx::pre_commit::unexpected_repo_interner_scope;

use super::api::ReplayToError;

/// Stage and validate the WAL at `wal_dir` against `v_star`. Applies
/// nothing; every refusal leaves the directory byte-identical.
pub(super) async fn stage_wal(
    wal_dir: &Path,
    v_star: u64,
) -> Result<Vec<WalEntryV2>, ReplayToError> {
    let entries = SegmentSet::read_entries_strict(wal_dir)
        .await
        .map_err(|error| match error {
            WalStrictReadError::IncompleteFrame { .. } => ReplayToError::TornTail(error),
            WalStrictReadError::Decode { .. } => ReplayToError::UndecodableEntry(error),
            WalStrictReadError::Other(DbError::NotFound(_)) => {
                ReplayToError::WalDirMissing(wal_dir.to_path_buf())
            }
            error => ReplayToError::WalRead(error),
        })?;

    validate_entries(&entries, v_star)?;
    Ok(entries)
}

/// Version and interner scope validation over decoded entries (no I/O).
fn validate_entries(entries: &[WalEntryV2], v_star: u64) -> Result<(), ReplayToError> {
    // Zero is the baseline for a valid segment set with no decoded records,
    // not evidence that any replay previously ran.
    let mut highest: u64 = 0;
    for entry in entries {
        let v = entry.commit_version;
        if v > v_star {
            return Err(ReplayToError::EntryAboveVStar { version: v, v_star });
        }
        highest = highest.max(v);
        for (scope, _, _) in &entry.interner_delta {
            if let Some(repo_scope) = unexpected_repo_interner_scope(*scope) {
                return Err(ReplayToError::InternerScopeUnprovable {
                    scope: *scope,
                    txn_id: entry.txn_id,
                    repo_scope,
                });
            }
        }
    }

    if highest != v_star {
        return Err(ReplayToError::HighestBelowVStar { highest, v_star });
    }

    // Check duplicates across all versions. Only positive versions form
    // the retained contiguous suffix; it need not start at version 1.
    let mut versions: Vec<u64> = entries.iter().map(|entry| entry.commit_version).collect();
    versions.sort_unstable();
    for pair in versions.windows(2) {
        if pair[0] == pair[1] {
            return Err(ReplayToError::DuplicateVersion(pair[0]));
        }
        if pair[0] > 0 && pair[1] - pair[0] != 1 {
            return Err(ReplayToError::VersionGap {
                below: pair[0],
                expected: pair[0] + 1,
                above: pair[1],
            });
        }
    }

    Ok(())
}
