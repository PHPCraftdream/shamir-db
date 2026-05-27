//! Unified `__meta__/*` namespace for engine metadata.
//!
//! Each variant maps to a deterministic `RecordId::system(name)` where
//! `name` is a short (≤12 bytes) ASCII tag. Coexists with existing
//! `system:*` keys — new code uses these, old code untouched.

use shamir_types::types::record_id::RecordId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetaKey {
    /// Unified index registry (replaces `system:indexes` +
    /// `system:indexes_unique` in the v2 layout).
    Indexes,
    /// Per-table schema, column info.
    Tables,
    /// WAL checkpoint / LSN state.
    Wal,
    /// Active migration coordinator state.
    Migrations,
    /// Interner state. Currently inline as RecordId::system("internals").
    Internals,
    /// Record counter persisted value. Inline as system("count").
    Count,
    /// MemBuffer config persistence. Inline as system("buffer_config").
    BufferConfig,
    /// SortedIndexManager registry. Inline as system("sorted_indexes").
    SortedIndexes,
    /// Legacy IndexManager regular-index registry.
    /// Inline as system("indexes").
    LegacyIndexes,
    /// Legacy IndexManager unique-index registry.
    /// Inline as system("indexes_unique").
    LegacyIndexesUnique,

    /// Last committed MVCC version (`u64 BE` body).
    ///
    /// Durable recovery marker. Read on repo open to seed
    /// `RepoTxGate::last_committed_version`. Written on every tx
    /// commit during phase 6 (publish). Without this marker, recovery
    /// can't tell which versions in `MvccStore::history` are visible.
    LastCommittedVersion,

    /// Periodic snapshot of next tx id (`u64 BE` body).
    ///
    /// Written every N commits (config, default 100). On recovery, the
    /// engine seeds the counter from `max(this, max(WAL active txn_id))`
    /// so issued tx ids never collide with already-committed ones.
    NextTxId,
}

impl MetaKey {
    /// Short ASCII tag stored after the 4-byte zero system-prefix.
    /// Max 12 bytes per `RecordId::system`.
    pub const fn tag(self) -> &'static str {
        match self {
            MetaKey::Indexes => "_m.idx",
            MetaKey::Tables => "_m.tbl",
            MetaKey::Wal => "_m.wal",
            MetaKey::Migrations => "_m.mig",
            MetaKey::Internals => "internals",
            MetaKey::Count => "count",
            MetaKey::BufferConfig => "buffer_config",
            MetaKey::SortedIndexes => "sorted_indexes",
            MetaKey::LegacyIndexes => "indexes",
            MetaKey::LegacyIndexesUnique => "indexes_unique",
            MetaKey::LastCommittedVersion => "_t.lcv",
            MetaKey::NextTxId => "_t.nti",
        }
    }

    pub fn as_record_id(self) -> RecordId {
        RecordId::system(self.tag())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[MetaKey] = &[
        MetaKey::Indexes,
        MetaKey::Tables,
        MetaKey::Wal,
        MetaKey::Migrations,
        MetaKey::Internals,
        MetaKey::Count,
        MetaKey::BufferConfig,
        MetaKey::SortedIndexes,
        MetaKey::LegacyIndexes,
        MetaKey::LegacyIndexesUnique,
        MetaKey::LastCommittedVersion,
        MetaKey::NextTxId,
    ];

    #[test]
    fn record_ids_are_distinct() {
        let mut rids: Vec<_> = ALL.iter().map(|k| k.as_record_id()).collect();
        let original_len = rids.len();
        rids.sort();
        rids.dedup();
        assert_eq!(
            rids.len(),
            original_len,
            "all MetaKey variants must produce distinct RecordIds (no truncation collision)"
        );
    }

    #[test]
    fn record_id_is_system() {
        for k in ALL {
            assert!(
                k.as_record_id().is_system(),
                "{:?} must be a system record",
                k
            );
        }
    }

    #[test]
    fn tags_match_legacy_literal_encoding() {
        // Each new MetaKey variant must produce EXACTLY the same
        // RecordId bytes as the inline literal it replaces. Otherwise
        // on-disk data persisted before this refactor becomes invisible.
        assert_eq!(
            MetaKey::Internals.as_record_id(),
            RecordId::system("internals")
        );
        assert_eq!(MetaKey::Count.as_record_id(), RecordId::system("count"));
        assert_eq!(
            MetaKey::BufferConfig.as_record_id(),
            RecordId::system("buffer_config")
        );
        assert_eq!(
            MetaKey::SortedIndexes.as_record_id(),
            RecordId::system("sorted_indexes")
        );
        assert_eq!(
            MetaKey::LegacyIndexes.as_record_id(),
            RecordId::system("indexes")
        );
        assert_eq!(
            MetaKey::LegacyIndexesUnique.as_record_id(),
            RecordId::system("indexes_unique")
        );
    }
}
