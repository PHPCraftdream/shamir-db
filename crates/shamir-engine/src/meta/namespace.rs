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
        }
    }

    pub fn as_record_id(self) -> RecordId {
        RecordId::system(self.tag())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_short_and_distinct() {
        let all = [
            MetaKey::Indexes,
            MetaKey::Tables,
            MetaKey::Wal,
            MetaKey::Migrations,
        ];
        for k in all {
            assert!(k.tag().len() <= 12, "{:?} tag too long", k);
        }
        // distinct
        let mut tags: Vec<&str> = all.iter().map(|k| k.tag()).collect();
        tags.sort();
        tags.dedup();
        assert_eq!(tags.len(), all.len());
    }

    #[test]
    fn record_id_is_system() {
        let rid = MetaKey::Indexes.as_record_id();
        assert!(rid.is_system());
    }
}
