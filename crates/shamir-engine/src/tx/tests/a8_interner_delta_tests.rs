//! A8 — interner-delta lost when the "first toucher" aborts before WAL.
//!
//! Reproduces the audit finding (A8, `docs/dev-artifacts/audits/2026-07-06-concurrency-
//! engine.md`): a later committer's records can reference an interner id
//! that some OTHER tx created (via `touch_ind`) above
//! `persisted_high_water()`, but that no surviving WAL delta mentions —
//! because the first toucher aborted before its WAL write. After a crash
//! before the next checkpoint, the later committer's records become
//! undecodable.
//!
//! The fix: every committer must include in its own `interner_deltas` ALL
//! `(name, id)` pairs referenced by its own staged bytes that are ABOVE
//! `persisted_high_water()` — not just the ids it happened to be the first
//! to create.

use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, StagingStore, TxContext, TxId};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::TMap;
use shamir_types::types::value::InnerValue;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::commit_tx;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

/// Build staged `InnerValue::Map` bytes whose single key is `key_id`
/// (an `InternerKey` id) mapping to a trivial `InnerValue::Str`. This
/// simulates a record that references a field name by its interned id.
fn map_bytes_with_key(key_id: u64) -> Bytes {
    let mut m = TMap::default();
    m.insert(InternerKey::new(key_id), InnerValue::Str("v".into()));
    InnerValue::Map(m).to_bytes().expect("encode succeeds")
}

/// A8 core reproduction: tx2 references an id (created in base by some
/// OTHER tx via `touch_ind`) that is ABOVE `persisted_high_water()` and
/// that tx2 did NOT create itself. Before the fix, tx2's
/// `interner_deltas` was empty (its overlay was empty / `is_new == false`)
/// and the WAL entry carried no `(name, id)` for that id — so after a
/// crash the record would be undecodable. After the fix, tx2's delta
/// includes the pair.
#[tokio::test]
async fn a8_referenced_id_above_hwm_recorded_in_delta() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("items"));
    let tbl = repo.get_table("items").await.unwrap();
    let interner = tbl.interner().get().await.unwrap();

    // Simulate "tx1 was the first to touch 'foo' in base, then aborted
    // before WAL" — the id exists in the in-memory base interner above
    // the persisted floor, but NO committed delta has ever carried it.
    let foo_id = interner.touch_ind("foo").expect("touch_ind ok").key().id();
    assert!(
        foo_id > tbl.interner().persisted_high_water() as u64,
        "foo_id ({foo_id}) must be above persisted_high_water for the test precondition"
    );

    // tx2 references id `foo_id` in its staged bytes but does NOT intern
    // "foo" itself (its overlay is empty). This mirrors the audit's
    // interleaving where tx2's `commit_interner_overlay` produces no
    // delta for "foo" because base already has it.
    let token = table_token_for("items");
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(Arc::clone(&data_store));
    let staged_bytes = map_bytes_with_key(foo_id);
    staging.set(Bytes::from_static(b"k").into(), staged_bytes);
    let mut tx = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Snapshot);
    tx.write_set.insert(token, staging);

    let wal = repo.repo_wal().await.unwrap();
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert!(outcome.materialized(), "tx2 must commit successfully");

    // Inspect the WAL entry's interner_delta for the (name, id) pair.
    let inflight = wal.recover().await.unwrap();
    let entry = inflight
        .iter()
        .find(|e| e.txn_id == outcome.tx_id)
        .expect("tx2's WAL entry must be present");
    let has_foo = entry
        .interner_delta
        .iter()
        .any(|(_scope, name, id)| name == "foo" && *id == foo_id);
    assert!(
        has_foo,
        "A8: tx2's WAL interner_delta must include (\"foo\", {foo_id}) \
         since tx2's records reference that id above persisted_high_water. \
         Got: {:?}",
        entry.interner_delta
    );
}

/// Regression guard: the existing `is_new`-based fast path still works —
/// a tx that IS the first to intern a brand-new name (via its overlay)
/// still gets `(name, id)` in its delta. Passes both before and after
/// the A8 fix.
#[tokio::test]
async fn a8_fast_path_first_interner_still_recorded() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("items"));
    let tbl = repo.get_table("items").await.unwrap();
    let interner = tbl.interner().get().await.unwrap();

    // tx interns a brand-new name via the overlay path: allocate an
    // overlay id, stage bytes referencing it, and populate the tx's
    // overlay so commit_interner_overlay will merge + remap it.
    let token = table_token_for("items");
    let overlay_id = shamir_tx::OVERLAY_ID_BASE;
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(Arc::clone(&data_store));
    let staged_bytes = map_bytes_with_key(overlay_id);
    staging.set(Bytes::from_static(b"k").into(), staged_bytes);
    let mut tx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    tx.interner_overlay
        .insert_sync("brand_new".to_string(), overlay_id)
        .expect("overlay insert ok");
    tx.write_set.insert(token, staging);

    let wal = repo.repo_wal().await.unwrap();
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert!(outcome.materialized());

    let inflight = wal.recover().await.unwrap();
    let entry = inflight
        .iter()
        .find(|e| e.txn_id == outcome.tx_id)
        .expect("WAL entry present");
    // The merged base id for "brand_new" — look it up post-commit.
    let base_id = interner
        .get_ind("brand_new")
        .expect("brand_new in base")
        .id();
    let has = entry
        .interner_delta
        .iter()
        .any(|(_scope, name, id)| name == "brand_new" && *id == base_id);
    assert!(
        has,
        "fast-path: first interner of 'brand_new' must still record (name, id) in delta"
    );
}

/// Regression guard: a tx whose staged bytes only reference ids ALREADY
/// below `persisted_high_water()` (already durably persisted) does NOT
/// grow `interner_deltas` for those — only ids above the floor are added.
#[tokio::test]
async fn a8_no_spurious_delta_for_persisted_ids() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("items"));
    let tbl = repo.get_table("items").await.unwrap();
    let interner = tbl.interner().get().await.unwrap();

    // Create an id and PERSIST it so it sits below the high-water mark.
    let persisted_id = interner.touch_ind("persisted").expect("ok").key().id();
    tbl.interner().persist().await.unwrap();
    let hwm = tbl.interner().persisted_high_water() as u64;
    assert!(
        persisted_id <= hwm,
        "persisted_id ({persisted_id}) must be at or below hwm ({hwm}) after persist"
    );

    // tx references ONLY the persisted id — no new interner work, no
    // above-hwm ids. Its delta must NOT mention "persisted".
    let token = table_token_for("items");
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(Arc::clone(&data_store));
    let staged_bytes = map_bytes_with_key(persisted_id);
    staging.set(Bytes::from_static(b"k").into(), staged_bytes);
    let mut tx = TxContext::new(TxId::new(3), 0, 0, IsolationLevel::Snapshot);
    tx.write_set.insert(token, staging);

    let wal = repo.repo_wal().await.unwrap();
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert!(outcome.materialized());

    let inflight = wal.recover().await.unwrap();
    let entry = inflight
        .iter()
        .find(|e| e.txn_id == outcome.tx_id)
        .expect("WAL entry present");
    let mentions_persisted = entry
        .interner_delta
        .iter()
        .any(|(_scope, name, _id)| name == "persisted");
    assert!(
        !mentions_persisted,
        "no spurious delta for already-persisted id; got: {:?}",
        entry.interner_delta
    );
}
