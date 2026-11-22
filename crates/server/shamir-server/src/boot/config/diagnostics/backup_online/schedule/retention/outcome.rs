/// Counts from one pass. `untouched` is available only after a complete scan.
#[derive(Debug, Default)]
pub(in crate::boot::config::diagnostics::backup_online) struct RetentionOutcome {
    pub(in crate::boot::config::diagnostics::backup_online) deleted: u64,
    pub(in crate::boot::config::diagnostics::backup_online) failed: u64,
    /// Snapshots left in place because another process held the read lease
    /// during this deletion attempt. Normal contention is not an error; a
    /// later pass may retry, but no retry or deletion is promised.
    pub(in crate::boot::config::diagnostics::backup_online) skipped_in_use: u64,
    pub(in crate::boot::config::diagnostics::backup_online) untouched: Option<u64>,
    /// Directory name, manifest creation time and manifest SHA-256 digest
    /// triples reported as adopted by this pass for entries in the registry's
    /// bounded in-memory provenance (`RunRecord::pending_adoptions`). A
    /// matching entry already bearing this owner's tag may be reported here
    /// without writing a new tag. The digest compares manifest bytes captured
    /// after protocol success, at scan and at the late check; it does not
    /// establish payload or directory identity. The caller retires exactly
    /// these triples because they are now owner-tagged or were already
    /// owner-tagged, regardless of later pass outcome.
    pub(in crate::boot::config::diagnostics::backup_online) adopted: Vec<(String, u128, String)>,
}
