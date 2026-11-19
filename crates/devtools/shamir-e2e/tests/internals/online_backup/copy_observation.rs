//! Shared status proof for a live copy observation (brief 50-r2a-4,
//! followup item 4; corrected: the tight gate proof moved to the live
//! `Copying` stage): parse a `BackupStatus` record's `progress.per_repo`
//! counters while the run is live in a chosen stage.
//!
//! Wire contract (`admin_port.rs`): stage names are the `Debug` names of
//! `BackupStage` (`"Copying"`, `"Sealing"`, `"Verifying"`, `"Done"`);
//! `progress` is a nested map whose `per_repo` is a list of `{repo,
//! bytes_hashed, files_hashed}` maps. While the protocol runs the
//! registry keeps `per_repo` EMPTY — it is filled only after the
//! protocol returns — so an empty list does NOT mean no live
//! measurement was taken; the per-repo fields of a live record are not
//! the actual live counters.
//!
//! Two selectors share one strictness contract:
//! - [`active_copy`] — the live `Copying` stage (the protocol's
//!   `begin_copy` hook publishes it once the capture barrier ends and
//!   before the first live WAL/store pass). Writers still commit during
//!   this stage, so it is the truthful home of the gate's tight
//!   write-overlap proof; the per-repo seal holds that follow admit no
//!   new commits by design, so `Sealing` can no longer bracket an
//!   acknowledged write. `Sealing` begins at the first per-repo seal and
//!   continues through the residual pass; it excludes the final
//!   verification and bookkeeping interval (the adapter sets `Verifying`
//!   at the start of that final stage, once all seals plus the residual
//!   pass have finished).
//! - [`active_seal_copy`] — the live `Sealing` stage, kept UNCHANGED for
//!   the churn test (whose churn write targets a repository the seal
//!   hold does not touch).
//!
//! A malformed `per_repo` list on an active stage yields `None` — never
//! treated as proof that no copy happened.
//!
//! Kept separate from `stage_is_live` in `churn_tests.rs` (which stays
//! unchanged); this module is the shared helper.

use std::time::{Duration, Instant};

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

/// Shared strictness contract of the two live selectors: same
/// `backup_id`, `running == true`, top stage `== stage_name`, then the
/// strictly parsed `progress.per_repo` amount (a malformed or missing
/// list yields `None` — callers must not treat that as proof of no
/// copy).
fn active_live_copy(
    status: &QueryRecord,
    backup_id: &str,
    stage_name: &str,
) -> Option<CopyProgress> {
    if status.get_value_str("backup_id") != Some(backup_id) {
        return None;
    }
    if status.get_value_bool("running") != Some(true) {
        return None;
    }
    if status.get_value_str("stage") != Some(stage_name) {
        return None;
    }
    CopyProgress::from_status(status)
}

/// `Some` when the status record shows THIS backup live in the `Copying`
/// stage: same `backup_id`, `running == true`, top stage `== "Copying"`
/// (published by the protocol's `begin_copy` hook once the capture
/// barrier ends and before the first live WAL/store pass; the adapter
/// flips it to `Sealing` at each per-repo seal through the residual
/// pass, and to `Verifying` only at the start of the final verification
/// stage, once all seals plus the residual pass have finished — the
/// stage alone is decisive). Writers still
/// commit during this stage, so it brackets acknowledged writes truthfully.
pub(super) fn active_copy(status: &QueryRecord, backup_id: &str) -> Option<CopyProgress> {
    active_live_copy(status, backup_id, "Copying")
}

/// `Some` when the status record shows THIS backup live in the `Sealing`
/// stage (kept UNCHANGED for the churn test, whose write targets a
/// repository the seal hold does not touch): same `backup_id`,
/// `running == true`, top stage `== "Sealing"` (the adapter sets
/// `Sealing` only immediately before each per-repo seal, never during
/// final bookkeeping — the stage alone is decisive).
pub(super) fn active_seal_copy(status: &QueryRecord, backup_id: &str) -> Option<CopyProgress> {
    active_live_copy(status, backup_id, "Sealing")
}

/// Hard bound on consecutive ack-to-ack gaps per writer (R3, §6.2): 10×
/// the design's 2 s p99 per-ack baseline plus CI scheduling slack.
pub(super) const ACK_GAP_BOUND: Duration = Duration::from_secs(30);

/// One qualifying status observation of the live COPY PASS: the wire
/// record showed THIS backup live in the `Copying` stage (see
/// [`active_copy`] — the per-repo seal holds, `Verifying` and terminal
/// bookkeeping are NOT live and never produce an observation; the seal
/// hold intentionally admits no new commits, so the tight write-overlap
/// proof lives on the first live WAL/store pass). Status-placeholder
/// note: while the protocol runs the registry keeps `per_repo` EMPTY
/// (parses to the zero progress), and an active copy with a malformed
/// list yields `None` from `active_copy` — neither is treated as proof
/// that no copy happened; the timestamps are kept regardless of the
/// amount.
pub(super) struct CopyObservation {
    /// `Instant` of the request send — taken BEFORE the request goes out.
    pub sent_at: Instant,
    /// `Instant` of the response receipt — set only AFTER the response
    /// arrived, never speculatively before send.
    pub received_at: Instant,
    /// Strictly parsed summed `per_repo` counters at poll time (may be
    /// the zero placeholder while the protocol runs).
    pub progress: CopyProgress,
}

/// Timeline edges of the observed copy window: the FIRST and the LAST
/// qualifying `Copying` observations, each carrying both its request-send
/// and response-receipt `Instant`s and the parsed [`CopyProgress`].
pub(super) struct StatusObservation {
    pub first: Option<CopyObservation>,
    pub last: Option<CopyObservation>,
}
