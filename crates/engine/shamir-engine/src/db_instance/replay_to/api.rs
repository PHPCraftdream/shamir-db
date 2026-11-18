//! `replay_to` entry point, report and typed refusals (R2a step 3b-1).

use std::path::{Path, PathBuf};

use shamir_storage::error::DbError;
use shamir_wal::{WalEntryV2, WalOpV2, WalStrictReadError};

use super::scope::{ReplayScope, ReplayScopeError};
use super::{open, staging};

/// Per-kind counts of staged WAL operations, including cold-skipped markers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayToOpCounters {
    pub put: u64,
    pub delete: u64,
    pub counter_delta: u64,
    pub index_put: u64,
    pub index_del: u64,
    pub interner_overlay_merge: u64,
}

impl ReplayToOpCounters {
    fn from_entries(entries: &[WalEntryV2]) -> Self {
        let mut counters = Self::default();
        for op in entries.iter().flat_map(|entry| &entry.ops) {
            match op {
                WalOpV2::Put { .. } => counters.put += 1,
                WalOpV2::Delete { .. } => counters.delete += 1,
                WalOpV2::CounterDelta { .. } => counters.counter_delta += 1,
                WalOpV2::IndexPut { .. } => counters.index_put += 1,
                WalOpV2::IndexDel { .. } => counters.index_del += 1,
                WalOpV2::InternerOverlayMerge { .. } => counters.interner_overlay_merge += 1,
            }
        }
        counters
    }

    /// Sum of all per-kind counters.
    pub fn total(&self) -> u64 {
        self.put
            + self.delete
            + self.counter_delta
            + self.index_put
            + self.index_del
            + self.interner_overlay_merge
    }
}

/// Successful strict cold replay of the entire retained WAL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    /// Complete entries accepted by the bounded WAL validation.
    pub entries_staged: u64,
    /// Number of entries applied by replay.
    pub entries_applied: u64,
    /// Highest applied commit version, or zero if none were applied.
    pub highest_version_applied: u64,
    /// The recorded point `V*`, verified by recovery on success.
    pub committed_version: u64,
    /// Per-kind op counters over the staged entries.
    pub counters: ReplayToOpCounters,
}

/// Typed failures from preflight or store open and recovery.
#[derive(Debug, thiserror::Error)]
pub enum ReplayToError {
    /// `wal_dir` is not the engine's WAL layout for `store_dir`
    /// (`<store_dir file_name>.shamirwal` sibling). Nothing was touched.
    #[error("wal_dir {wal_dir:?} is not the WAL sibling of store_dir {store_dir:?} (expected {expected:?})")]
    WalPathMismatch {
        store_dir: PathBuf,
        wal_dir: PathBuf,
        expected: PathBuf,
    },
    /// The WAL directory holds no recognized segments.
    #[error("wal_dir {0:?} holds no WAL segments")]
    WalDirMissing(PathBuf),
    /// The store directory is missing. Nothing was touched.
    #[error("store_dir {0:?} does not exist")]
    StoreDirMissing(PathBuf),
    /// An incomplete frame was found in a staged WAL segment.
    #[error("staged WAL contains a torn frame: {0}")]
    TornTail(#[source] WalStrictReadError),
    /// A checksum-valid WAL frame failed to decode.
    #[error("staged WAL contains an undecodable entry: {0}")]
    UndecodableEntry(#[source] WalStrictReadError),
    /// Another strict WAL read failure (e.g. directory or file I/O, or a bad checksum).
    #[error("strict staged WAL read failed: {0}")]
    WalRead(#[source] WalStrictReadError),
    /// A staged entry's version is ABOVE the recorded point `V*`.
    #[error("staged WAL carries entry version {version} above the recorded point V* = {v_star}")]
    EntryAboveVStar { version: u64, v_star: u64 },
    /// The highest complete staged entry is BELOW `V*` (the copy was sealed
    /// at `V*`; a shorter WAL cannot reproduce the sealed state). A WAL
    /// with no decoded records uses zero as its baseline.
    #[error("highest staged entry version {highest} is below the recorded point V* = {v_star}")]
    HighestBelowVStar { highest: u64, v_star: u64 },
    /// The same `commit_version` appears in more than one staged entry.
    #[error("staged WAL carries duplicate commit_version {0}")]
    DuplicateVersion(u64),
    /// A missing positive version within the retained positive-version suffix.
    #[error("staged WAL has a version gap: {expected} is missing between {below} and {above}")]
    VersionGap {
        below: u64,
        expected: u64,
        above: u64,
    },
    /// An interner delta whose scope is not the repo scope â€” it cannot be
    /// proven to belong to this repository.
    #[error("staged WAL carries interner delta scope {scope} (txn_id {txn_id}); only scope {repo_scope} is provable")]
    InternerScopeUnprovable {
        scope: u64,
        txn_id: u64,
        repo_scope: u64,
    },
    /// The supplied catalogue could not route the staged WAL in memory.
    #[error("staged WAL scope validation failed: {0}")]
    Scope(#[source] ReplayScopeError),
    /// Replay is unsupported in this build. No store was opened.
    #[error("offline WAL replay requires the fjall feature")]
    UnsupportedBackend,
    /// Exclusive store open failed; the backend may already have written data.
    #[error("exclusive offline store open failed: {0}")]
    Open(#[source] DbError),
    /// Strict recovery failed after store open; the store may have changed.
    #[error("offline WAL recovery failed: {0}")]
    Recovery(#[source] DbError),
}

/// Strictly cold-replay an offline Fjall copy to the recorded point `V*`.
///
/// The caller must keep both paths offline and stable throughout. `wal_dir`
/// must be the `<store_dir file_name>.shamirwal` sibling. The supplied scope
/// must come from the same catalogue as normal boot.
///
/// Path, WAL and scope preflight are read-only. The store is guaranteed to
/// stay byte-identical only on failures before store open is attempted;
/// neither [`ReplayToError::Open`] nor [`ReplayToError::Recovery`] implies rollback.
/// Cancelling after store open begins may also leave changes on the copy.
///
/// Every retained entry is replayed, including entries at or below the store
/// version floor. Ordinary `CounterDelta` markers are counted in the report
/// but skipped by cold recovery. Replay retains the WAL, so a successful
/// repeat against the same copy and scope returns the same report.
pub async fn replay_to(
    store_dir: &Path,
    wal_dir: &Path,
    v_star: u64,
    scope: ReplayScope,
) -> Result<ReplayReport, ReplayToError> {
    // Match the sibling layout derived by `durability_access.rs::repo_wal`.
    let Some(name) = store_dir.file_name() else {
        return Err(ReplayToError::StoreDirMissing(store_dir.to_path_buf()));
    };
    let mut wal_name = name.to_os_string();
    wal_name.push(".shamirwal");
    let expected = store_dir.with_file_name(wal_name);
    if wal_dir != expected {
        return Err(ReplayToError::WalPathMismatch {
            store_dir: store_dir.to_path_buf(),
            wal_dir: wal_dir.to_path_buf(),
            expected,
        });
    }
    if !store_dir.is_dir() {
        return Err(ReplayToError::StoreDirMissing(store_dir.to_path_buf()));
    }

    let entries = Box::pin(staging::stage_wal(wal_dir, v_star)).await?;
    scope.validate(&entries).map_err(ReplayToError::Scope)?;
    let entries_staged = entries.len() as u64;
    let counters = ReplayToOpCounters::from_entries(&entries);
    let entries_applied = Box::pin(open::replay(
        name.to_string_lossy().into_owned(),
        store_dir,
        scope.into_configs(),
        entries,
        v_star,
    ))
    .await? as u64;
    Ok(ReplayReport {
        entries_staged,
        entries_applied,
        highest_version_applied: v_star,
        committed_version: v_star,
        counters,
    })
}
