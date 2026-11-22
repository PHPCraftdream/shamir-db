/// Counts from one pass. `untouched` is available only after a complete scan.
#[derive(Debug, Default)]
pub(in crate::boot::config::diagnostics::backup_online) struct RetentionOutcome {
    pub(in crate::boot::config::diagnostics::backup_online) deleted: u64,
    pub(in crate::boot::config::diagnostics::backup_online) failed: u64,
    /// Snapshots left in place because another process holds the read lease.
    /// Normal contention, not an error: the next pass retries.
    pub(in crate::boot::config::diagnostics::backup_online) skipped_in_use: u64,
    pub(in crate::boot::config::diagnostics::backup_online) untouched: Option<u64>,
    /// Exact (directory name, manifest creation time) pairs this pass
    /// TAGGED as fresh adoptable outputs recorded in the registry's bounded
    /// IN-MEMORY provenance (`RunRecord::pending_adoptions`): outputs the
    /// registry published while the lease marker was broken. The caller
    /// drops exactly these reported pairs from that list once reported,
    /// whatever the rest of the pass did — the ownership tag is already
    /// durable. The pair (not the bare name) is what is reported and
    /// retired, so a different output that later occupies a recorded name
    /// is never mistaken for the recorded one.
    pub(in crate::boot::config::diagnostics::backup_online) adopted: Vec<(String, u128)>,
}
