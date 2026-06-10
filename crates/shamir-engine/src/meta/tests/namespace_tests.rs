use crate::meta::namespace::MetaKey;
use shamir_types::types::record_id::RecordId;

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
    MetaKey::Validators,
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
