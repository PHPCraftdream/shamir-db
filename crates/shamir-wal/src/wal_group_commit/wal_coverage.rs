//! Injectable exact-version coverage; implementations own sparse prefix state.

/// A repository-owned recoverability tracker. Callbacks are synchronous and
/// must be Send + Sync. `written` is called only by the sync worker after
/// replay of complete bytes, never by a successful production append leader.
/// `capture` precedes fsync; `synced` is called only on successful fsync.
/// A maximum appended version is never a coverage prefix.
pub trait WalCoverage: Send + Sync {
    /// Sync-worker publication only; versions are an exact pre-fsync snapshot.
    fn written(&self, versions: &[u64], memory: bool);
    /// Existing completion W is a safe RAM-only prefix (including applied data).
    fn memory_completed(&self, _through: u64) {}
    fn enable_memory(&self) {}
    fn capture(&self) -> u64;
    fn synced(&self, generation: u64);
    fn settle(&self, version: u64);
    /// Initialize the recovered prefix ONLY after successful exclusive cold-open
    /// replay, history flush and conservative journal certification, before any
    /// commit/apply admission. An allocation reservation alone is NOT evidence.
    /// Never call on live first touch or to cover an outstanding live write.
    /// Default keeps coverage unchanged (fail-closed for observers that do not
    /// support recovery initialization); it never grants a live prefix claim.
    fn certify_exclusive_open(&self, _through: u64) {}
    fn recoverable_through(&self) -> u64;
}
