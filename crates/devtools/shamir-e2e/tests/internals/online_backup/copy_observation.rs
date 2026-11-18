//! Shared status proof for the live copy window (brief 50-r2a-4, followup
//! item 4): parse a `BackupStatus` record's `progress.per_repo` counters
//! while the run is live in the `Sealing` stage.
//!
//! Wire contract (`admin_port.rs`): stage names are the `Debug` names of
//! `BackupStage` (`"Sealing"`, `"Verifying"`, `"Done"`); `progress` is a
//! nested map whose `per_repo` is a list of `{repo, bytes_hashed,
//! files_hashed}` maps. The registry keeps `per_repo` EMPTY while the
//! protocol runs and fills it only after the protocol returns — so an empty
//! list does NOT mean no live measurement was taken; the per-repo fields of
//! a live record are not the actual live counters. The `Sealing` stage
//! excludes the final bookkeeping interval (the adapter sets it only before
//! each per-repo seal): a write acknowledged between two `Sealing`
//! observations of one run was acknowledged before the seal phase ended.
//! This is NOT file-level proof that the write overlapped file copying: the
//! live adapter sets `Sealing` before the seal barrier and publishes no
//! live byte counters, so the bracket can also fall between two barriers.
//!
//! Kept separate from `stage_is_live` in `gate_tests.rs` / `churn_tests.rs`
//! (those stay unchanged); this module is the followup shared helper.

use shamir_query_types::read::query_record::QueryRecord;

/// Per-repo hashed counters summed over `progress.per_repo` of one
/// `BackupStatus` record (the copy amount observed at poll time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CopyProgress {
    pub bytes_hashed: u64,
    pub files_hashed: u64,
}

impl CopyProgress {
    /// Strictly parse `progress.per_repo`: `None` when `progress` or
    /// `per_repo` is missing or not a list, or when ANY entry is malformed
    /// (not a map, or its `bytes_hashed` / `files_hashed` is missing or not
    /// an integer) — malformed wire shape is never reported as a zero
    /// amount. A valid empty list parses to the zero progress (the registry
    /// legitimately reports an empty list live; see the module doc).
    pub(super) fn from_status(status: &QueryRecord) -> Option<Self> {
        let per_repo = status.get_value("progress")?.get("per_repo")?.as_array()?;
        let mut progress = CopyProgress {
            bytes_hashed: 0,
            files_hashed: 0,
        };
        for entry in per_repo {
            let bytes = entry.get("bytes_hashed")?.as_u64()?;
            let files = entry.get("files_hashed")?.as_u64()?;
            progress.bytes_hashed += bytes;
            progress.files_hashed += files;
        }
        Some(progress)
    }

    /// The observed amount is strictly below a nonzero final amount on both
    /// counters — this poll happened before the run's final accounting, not
    /// after completion (it does not prove file-level copy overlap).
    ///
    /// # Panics
    /// When either counter is not strictly below the corresponding nonzero
    /// final counter.
    pub(super) fn assert_before_final(self, final_progress: CopyProgress) {
        assert!(
            self.bytes_hashed < final_progress.bytes_hashed && final_progress.bytes_hashed > 0,
            "live copy bytes {} must be below nonzero final {}",
            self.bytes_hashed,
            final_progress.bytes_hashed
        );
        assert!(
            self.files_hashed < final_progress.files_hashed && final_progress.files_hashed > 0,
            "live copy files {} must be below nonzero final {}",
            self.files_hashed,
            final_progress.files_hashed
        );
    }
}

/// `Some` when the status record shows THIS backup live in the `Sealing`
/// stage: same `backup_id`, `running == true`, top stage `== "Sealing"`
/// (the adapter sets `Sealing` only immediately before each per-repo seal,
/// never during final bookkeeping — the stage alone is decisive). The
/// amount is strictly parsed from `progress.per_repo`; a malformed or
/// missing list yields `None` even though the seal is active (callers must
/// not treat that as proof of no copy).
pub(super) fn active_seal_copy(status: &QueryRecord, backup_id: &str) -> Option<CopyProgress> {
    if status.get_value_str("backup_id") != Some(backup_id) {
        return None;
    }
    if status.get_value_bool("running") != Some(true) {
        return None;
    }
    if status.get_value_str("stage") != Some("Sealing") {
        return None;
    }
    CopyProgress::from_status(status)
}
