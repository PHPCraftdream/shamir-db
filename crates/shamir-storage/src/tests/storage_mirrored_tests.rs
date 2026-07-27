#![allow(deprecated)]

use crate::storage_in_memory::InMemoryStore;
use crate::storage_mirrored::{is_durable_table_config, MirroredStore};
use crate::tests::types_tests::collect_stream;
use crate::types::{RecordKey, Store};
use bytes::Bytes;
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;

/// A representative classified key — a `MetaKey::LegacyIndexes`-shaped tag
/// (`"indexes"`), matching the exact `RecordId::system` encoding.
fn classified_key() -> RecordKey {
    RecordKey::from_slice(RecordId::system("indexes").as_bytes())
}

/// A representative unclassified key — an ordinary (non-system) 16-byte
/// key, shaped like a real row id (`RecordId::new()`, non-zero
/// timestamp prefix) so it does NOT accidentally match the system-record
/// shape.
fn unclassified_key() -> RecordKey {
    RecordKey::from_slice(RecordId::new().as_bytes())
}

#[tokio::test]
async fn classified_key_set_visible_in_mirror_and_survives_hydration() {
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let key = classified_key();
    let value = Bytes::from_static(b"classified-value");
    store.set(key.clone(), value.clone()).await.unwrap();

    // Visible via the facade.
    assert_eq!(store.get(key.clone()).await.unwrap(), value);

    // Visible in the underlying mirror directly, not just via the facade.
    assert_eq!(mirror.get(key.clone()).await.unwrap(), value);

    // A FRESH MirroredStore over the SAME mirror hydrates the value back.
    let reopened = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();
    assert_eq!(reopened.get(key).await.unwrap(), value);
}

#[tokio::test]
async fn unclassified_key_set_absent_from_mirror_and_gone_after_hydration() {
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let key = unclassified_key();
    let value = Bytes::from_static(b"ephemeral-value");
    store.set(key.clone(), value.clone()).await.unwrap();

    // Visible via the facade (primary always gets the write).
    assert_eq!(store.get(key.clone()).await.unwrap(), value);

    // Absent from the underlying mirror directly.
    assert!(mirror.get(key.clone()).await.is_err());

    // A fresh MirroredStore over the same mirror does NOT see it —
    // it never persisted, so hydration cannot bring it back.
    let reopened = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();
    assert!(reopened.get(key).await.is_err());
}

#[tokio::test]
async fn remove_of_classified_key_removes_from_mirror_too() {
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let key = classified_key();
    store
        .set(key.clone(), Bytes::from_static(b"v"))
        .await
        .unwrap();
    assert!(mirror.get(key.clone()).await.is_ok());

    let removed = store.remove(key.clone()).await.unwrap();
    assert!(removed);

    // Gone from the facade...
    assert!(store.get(key.clone()).await.is_err());
    // ...and gone from the mirror, so a dropped index definition can't
    // resurrect on the next hydration.
    assert!(mirror.get(key.clone()).await.is_err());

    let reopened = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();
    assert!(reopened.get(key).await.is_err());
}

#[tokio::test]
async fn scan_prefix_stream_after_hydration_returns_interner_chunks() {
    // Mirrors the interner_manager.rs usage: chunks live at
    // `RecordId::system("i.d" + 9-digit zero-padded index)`, scanned
    // via `scan_prefix_stream("\0\0\0\0i.d", ...)`.
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let chunk0 = RecordKey::from_slice(RecordId::system("i.d000000000").as_bytes());
    let chunk1 = RecordKey::from_slice(RecordId::system("i.d000000001").as_bytes());
    store
        .set(chunk0.clone(), Bytes::from_static(b"chunk-0"))
        .await
        .unwrap();
    store
        .set(chunk1.clone(), Bytes::from_static(b"chunk-1"))
        .await
        .unwrap();

    // Sanity: both chunks are classified as durable and landed in mirror.
    assert!(mirror.get(chunk0.clone()).await.is_ok());
    assert!(mirror.get(chunk1.clone()).await.is_ok());

    let reopened = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let mut prefix = vec![0u8, 0, 0, 0];
    prefix.extend_from_slice(b"i.d");
    let results = collect_stream(reopened.scan_prefix_stream(Bytes::from(prefix), 16))
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert!(results
        .iter()
        .any(|(k, v)| *k == chunk0 && v.as_ref() == b"chunk-0"));
    assert!(results
        .iter()
        .any(|(k, v)| *k == chunk1 && v.as_ref() == b"chunk-1"));
}

#[tokio::test]
async fn insert_never_writes_to_mirror() {
    // `InMemoryStore::insert` mints its key from `RecordId::new()`
    // (non-zero timestamp prefix) — it can never match the
    // `[0,0,0,0]`-prefixed system-record shape the classifier requires,
    // so `insert` must never cause a mirror write.
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let key = store.insert(Bytes::from_static(b"row")).await.unwrap();
    assert!(!is_durable_table_config(&key));
    assert!(mirror.get(key.clone()).await.is_err());
    assert_eq!(store.get(key).await.unwrap(), Bytes::from_static(b"row"));
}

#[tokio::test]
async fn batch_apis_split_classified_and_unclassified_correctly() {
    // Exercises the trait's DEFAULT `set_many`/`remove_many`/`transact`
    // loops (MirroredStore does not override them) through a mixed
    // batch, proving the per-item classify-and-mirror behavior these
    // defaults inherit from `set`/`remove` composes correctly across a
    // batch call, not just single calls (tests 1-2 only exercise the
    // latter).
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let classified = classified_key();
    let unclassified = unclassified_key();

    let items = vec![
        (classified.clone(), Bytes::from_static(b"cfg")),
        (unclassified.clone(), Bytes::from_static(b"row")),
    ];
    let flags = store.set_many(items).await.unwrap();
    assert_eq!(flags, vec![true, true]);

    assert!(mirror.get(classified.clone()).await.is_ok());
    assert!(mirror.get(unclassified.clone()).await.is_err());

    let remove_flags = store
        .remove_many(vec![classified.clone(), unclassified.clone()])
        .await
        .unwrap();
    assert_eq!(remove_flags, vec![true, true]);
    assert!(mirror.get(classified).await.is_err());
}

/// Mirrors EVERY `MetaKey` variant's `(name, tag, expected_durable)` from
/// `crates/shamir-engine/src/meta/namespace.rs` as of this test's
/// writing.
///
/// `shamir-storage` cannot import `shamir-engine`'s real `MetaKey` enum
/// here — `shamir-engine` depends on `shamir-storage`, so the reverse
/// import would be a crate dependency cycle. This hand-copied list is
/// therefore the closest exhaustiveness guard achievable from inside
/// `shamir-storage` alone.
///
/// **MAINTENANCE CONTRACT:** any time a `MetaKey` variant is added,
/// removed, or renamed in `namespace.rs`, this list MUST be updated in
/// the same change — that is what makes this guard meaningful. Cross-
/// checked variant-by-variant against `namespace.rs` at the time this
/// test was written (14 variants): `Indexes` ("_m.idx"), `Tables`
/// ("_m.tbl"), `Wal` ("_m.wal"), `Migrations` ("_m.mig"), `Internals`
/// ("internals"), `Count` ("count"), `BufferConfig` ("buffer_config"),
/// `SortedIndexes` ("sorted_indexes"), `LegacyIndexes` ("indexes"),
/// `LegacyIndexesUnique` ("indexes_unique"), `LastCommittedVersion`
/// ("_t.lcv"), `NextTxId` ("_t.nti"), `Validators` ("_m.val"),
/// `ReplicationBookmark` ("_t.rbm").
const ALL_META_KEY_TAGS: &[(&str, bool)] = &[
    ("_m.idx", true),         // MetaKey::Indexes
    ("_m.tbl", true),         // MetaKey::Tables
    ("_m.wal", true),         // MetaKey::Wal
    ("_m.mig", true),         // MetaKey::Migrations
    ("internals", true),      // MetaKey::Internals
    ("count", false),         // MetaKey::Count — derived from row data
    ("buffer_config", true),  // MetaKey::BufferConfig
    ("sorted_indexes", true), // MetaKey::SortedIndexes
    ("indexes", true),        // MetaKey::LegacyIndexes
    ("indexes_unique", true), // MetaKey::LegacyIndexesUnique
    // Derived recovery/replication bookkeeping tied to the committed
    // transaction history — same "derived from data, not
    // configuration" hazard as `Count`. See `is_durable_table_config`'s
    // doc for the full rationale.
    ("_t.lcv", false), // MetaKey::LastCommittedVersion
    ("_t.nti", false), // MetaKey::NextTxId
    ("_m.val", true),  // MetaKey::Validators
    ("_t.rbm", false), // MetaKey::ReplicationBookmark
];

#[tokio::test]
async fn classifier_exhaustiveness_guard_against_every_meta_key() {
    // The most important test in this module: for every known
    // `MetaKey` tag (see `ALL_META_KEY_TAGS`'s doc for why this can't
    // be the live enum), assert the classifier's verdict is correct —
    // especially that `Count` and the MVCC/replication recovery
    // markers are excluded. A future `MetaKey` addition that forgets
    // to update the classifier (and this list) will not be caught
    // automatically by this test alone — the maintenance contract
    // above is the enforcement mechanism.
    for &(tag, expect_durable) in ALL_META_KEY_TAGS {
        let key = RecordKey::from_slice(RecordId::system(tag).as_bytes());
        let verdict = is_durable_table_config(&key);
        assert_eq!(
            verdict, expect_durable,
            "tag {:?} classified as {}, expected {}",
            tag, verdict, expect_durable
        );
    }

    // Representative NON-system keys must all be `false` — none of
    // them are even 16 bytes with a `[0,0,0,0]` prefix.

    // A posting key shape: 4 (index_id LE u32) + 1 (type_tag) + value
    // bytes + 16 (RecordId) — never exactly 16 bytes total for any
    // non-empty value, and even a contrived 11-byte value making the
    // total 32 bytes still fails the length check.
    let mut posting_key = vec![0u8; 4]; // index_id = 0 (LE)
    posting_key.push(0); // type_tag
    posting_key.extend_from_slice(b"value"); // value bytes
    posting_key.extend_from_slice(RecordId::new().as_bytes()); // 16-byte rid
    assert!(!is_durable_table_config(&RecordKey::from_slice(
        &posting_key
    )));

    // A sorted-index / vector-snapshot key shape: ASCII string keys
    // like "<keyspace>.g0.data.000000" — never 16 bytes, never a
    // `[0,0,0,0]` prefix.
    let sorted_index_key = RecordKey::from_slice(b"my_index.g0.data.000000");
    assert!(!is_durable_table_config(&sorted_index_key));

    // `_m.idx.lfv` — the legacy posting-format version stamp
    // (`shamir_index::persistence`, ~line 44/94-123) — is NOT a `MetaKey`
    // variant (it's an ad-hoc `RecordId::system(...)` call directly in
    // `shamir-index`, outside `namespace.rs`), so `ALL_META_KEY_TAGS`
    // above doesn't cover it. Asserted separately: static config (marks
    // the on-disk posting format current), not derived from row data —
    // must be durable, same as the other index-definition tags.
    let lfv_key = RecordKey::from_slice(RecordId::system("_m.idx.lfv").as_bytes());
    assert!(
        is_durable_table_config(&lfv_key),
        "_m.idx.lfv (legacy posting-format version stamp) must be durable"
    );

    let vector_snapshot_key = RecordKey::from_slice(b"vec_idx.g3.sidecar");
    assert!(!is_durable_table_config(&vector_snapshot_key));

    // A plain ordinary row id (RecordId::new(), non-zero timestamp
    // prefix) must also be false — same length as a system key (16
    // bytes) but fails the `[0,0,0,0]` prefix check.
    let row_key = RecordKey::from_slice(RecordId::new().as_bytes());
    assert!(!is_durable_table_config(&row_key));
}
