/// Counts from one pass. `untouched` is available only after a complete scan.
#[derive(Debug, Default)]
pub(in crate::boot::config::diagnostics::backup_online) struct RetentionOutcome {
    pub(in crate::boot::config::diagnostics::backup_online) deleted: u64,
    pub(in crate::boot::config::diagnostics::backup_online) failed: u64,
    /// Snapshots left in place because another process holds the read lease.
    /// Normal contention, not an error: the next pass retries.
    pub(in crate::boot::config::diagnostics::backup_online) skipped_in_use: u64,
    pub(in crate::boot::config::diagnostics::backup_online) untouched: Option<u64>,
}
