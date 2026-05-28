//! Trait abstraction for SSI read-set validation version lookup.
//!
//! `commit_tx` Phase 2 walks `TxContext.read_set` and calls
//! `version_of(table_id, key)` to detect concurrent writes — if the
//! current committed version exceeds the version the tx saw on read,
//! abort with SsiConflict.
//!
//! Wiring a real provider to executor/repo machinery requires per-
//! table MvccStore mapping; that lands with Stage 5 reconciliation.
//! For now this trait is the injection point — tests can set a mock
//! provider via `TxContext::set_version_provider`.

use bytes::Bytes;

pub trait VersionProvider: Send + Sync {
    /// Return the current committed version for `(table_id, key)`.
    /// Returning `0` means "no version tracked" — the read sees
    /// whatever's in main store, version_seen would also be ≤ 0,
    /// validation passes trivially.
    fn version_of(&self, table_id: u64, key: &Bytes) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct StubProvider {
        version: u64,
    }

    impl VersionProvider for StubProvider {
        fn version_of(&self, _table_id: u64, _key: &Bytes) -> u64 {
            self.version
        }
    }

    #[test]
    fn stub_provider_returns_configured_version() {
        let p: Arc<dyn VersionProvider> = Arc::new(StubProvider { version: 99 });
        assert_eq!(p.version_of(0, &Bytes::from_static(b"k")), 99);
    }
}
