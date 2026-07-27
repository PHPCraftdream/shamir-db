//! F-33 Step 2 (#836): `BoxRepo::Hybrid` / `BoxRepoFactory::Hybrid` routing
//! tests. Covers the 6-name routing table, `backing_dir() == None`,
//! `store_delete` removing from both tiers, and the "unrecognized name"
//! ephemeral fallback.

use crate::repo::repo_types::{BoxRepo, BoxRepoFactory, RepoFactory};
use bytes::Bytes;
use shamir_storage::storage_fjall::FjallRepo;
use shamir_storage::types::{RecordKey, Repo};
use shamir_types::types::record_id::RecordId;

/// A representative durable-config key, shaped exactly like
/// `is_durable_table_config` expects (`MetaKey::LegacyIndexes`'s tag).
fn classified_key() -> RecordKey {
    RecordKey::from_slice(RecordId::system("indexes").as_bytes())
}

async fn build_hybrid(info_path: &std::path::Path) -> BoxRepo {
    BoxRepoFactory::hybrid(info_path).create().await.unwrap()
}

// ============================================================================
// 1. Per-name routing: `__info__`/`__interner__` reach the disk tier;
//    the other 4 known names never do.
// ============================================================================

#[tokio::test]
async fn info_store_is_visible_in_disk_tier_for_a_classified_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo = build_hybrid(temp_dir.path()).await;

    let store = repo.store_get("__info__users").await.unwrap();
    let key = classified_key();
    let value = Bytes::from_static(b"index-def");
    store.set(key.clone(), value.clone()).await.unwrap();

    // Confirm it reached the disk tier directly, bypassing the routed
    // facade — fjall holds an exclusive lock on the directory, so BOTH
    // the routed `store` handle (an `Arc<FjallStore>` under the hood,
    // holding its own `Arc<Database>` clone) AND the hybrid repo itself
    // must be dropped first before a fresh `FjallRepo::new` can reopen
    // the SAME path.
    drop(store);
    drop(repo);
    let disk = FjallRepo::new(temp_dir.path()).unwrap();
    let disk_store = disk.store_get("__info__users").await.unwrap();
    assert_eq!(disk_store.get(key).await.unwrap(), value);
}

#[tokio::test]
async fn interner_store_is_visible_in_disk_tier_for_any_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo = build_hybrid(temp_dir.path()).await;

    let store = repo.store_get("__interner__").await.unwrap();
    // Allow-ALL classifier: even a non-system-shaped key must mirror.
    let key = RecordKey::from_slice(RecordId::new().as_bytes());
    let value = Bytes::from_static(b"interner-chunk");
    store.set(key.clone(), value.clone()).await.unwrap();

    drop(store);
    drop(repo);
    let disk = FjallRepo::new(temp_dir.path()).unwrap();
    let disk_store = disk.store_get("__interner__").await.unwrap();
    assert_eq!(disk_store.get(key).await.unwrap(), value);
}

#[tokio::test]
async fn history_store_never_reaches_disk_tier() {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo = build_hybrid(temp_dir.path()).await;

    let store = repo.store_get("__history__users").await.unwrap();
    let key = classified_key();
    let value = Bytes::from_static(b"row-data");
    store.set(key.clone(), value.clone()).await.unwrap();

    drop(store);
    drop(repo);
    let disk = FjallRepo::new(temp_dir.path()).unwrap();
    let disk_store = disk.store_get("__history__users").await.unwrap();
    assert!(disk_store.get(key).await.is_err());
}

#[tokio::test]
async fn data_store_never_reaches_disk_tier() {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo = build_hybrid(temp_dir.path()).await;

    let store = repo.store_get("__data__users").await.unwrap();
    let key = classified_key();
    let value = Bytes::from_static(b"row-data");
    store.set(key.clone(), value.clone()).await.unwrap();

    drop(store);
    drop(repo);
    let disk = FjallRepo::new(temp_dir.path()).unwrap();
    let disk_store = disk.store_get("__data__users").await.unwrap();
    assert!(disk_store.get(key).await.is_err());
}

#[tokio::test]
async fn tx_store_never_reaches_disk_tier() {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo = build_hybrid(temp_dir.path()).await;

    let store = repo.store_get("__tx__").await.unwrap();
    let key = classified_key();
    let value = Bytes::from_static(b"recovery-marker");
    store.set(key.clone(), value.clone()).await.unwrap();

    drop(store);
    drop(repo);
    let disk = FjallRepo::new(temp_dir.path()).unwrap();
    let disk_store = disk.store_get("__tx__").await.unwrap();
    assert!(disk_store.get(key).await.is_err());
}

#[tokio::test]
async fn changelog_store_never_reaches_disk_tier() {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo = build_hybrid(temp_dir.path()).await;

    let store = repo.store_get("__changelog__").await.unwrap();
    let key = classified_key();
    let value = Bytes::from_static(b"changefeed-event");
    store.set(key.clone(), value.clone()).await.unwrap();

    drop(store);
    drop(repo);
    let disk = FjallRepo::new(temp_dir.path()).unwrap();
    let disk_store = disk.store_get("__changelog__").await.unwrap();
    assert!(disk_store.get(key).await.is_err());
}

// ============================================================================
// 2. Memoization: the same store name resolves to the SAME instance.
// ============================================================================

#[tokio::test]
async fn store_get_memoizes_per_name() {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo = build_hybrid(temp_dir.path()).await;

    let store1 = repo.store_get("__info__users").await.unwrap();
    let key = classified_key();
    let value = Bytes::from_static(b"v1");
    store1.set(key.clone(), value.clone()).await.unwrap();

    // A second `store_get` on the SAME name must return a handle to the
    // SAME in-memory primary (not a freshly re-hydrated, empty one) — the
    // write above must be visible.
    let store2 = repo.store_get("__info__users").await.unwrap();
    assert_eq!(store2.get(key).await.unwrap(), value);
}

// ============================================================================
// 3. `backing_dir()` is `None` for a hybrid factory.
// ============================================================================

#[test]
fn backing_dir_is_none_for_hybrid_factory() {
    let temp_dir = tempfile::tempdir().unwrap();
    let factory = BoxRepoFactory::hybrid(temp_dir.path());
    assert_eq!(factory.backing_dir(), None);
}

// ============================================================================
// 4. `store_delete` removes an `__info__` entry from BOTH tiers.
// ============================================================================

#[tokio::test]
async fn store_delete_on_info_name_removes_from_both_tiers() {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo = build_hybrid(temp_dir.path()).await;

    let store = repo.store_get("__info__users").await.unwrap();
    let key = classified_key();
    let value = Bytes::from_static(b"index-def");
    store.set(key.clone(), value.clone()).await.unwrap();

    let deleted = repo.store_delete("__info__users").await.unwrap();
    assert!(deleted);

    // A freshly `store_get`'d instance under the same name (still on the
    // SAME live `repo`, so no re-hydration happens — this is really just
    // confirming the in-memory tier no longer has the entry either) must
    // NOT see the deleted value.
    let reopened = repo.store_get("__info__users").await.unwrap();
    assert!(reopened.get(key.clone()).await.is_err());

    // Drop every handle that holds an `Arc<Database>` clone (the original
    // `store`, the `reopened` handle, and the hybrid repo itself) to
    // release fjall's exclusive directory lock, then open a raw
    // FjallRepo over the SAME path to confirm the durable copy is ALSO
    // gone (not just the in-memory one).
    drop(store);
    drop(reopened);
    drop(repo);
    let disk = FjallRepo::new(temp_dir.path()).unwrap();
    let disk_store = disk.store_get("__info__users").await.unwrap();
    assert!(disk_store.get(key).await.is_err());
}

// ============================================================================
// 5. Unrecognized store name falls back to ephemeral in-memory rather
//    than erroring.
// ============================================================================

// `HybridRepoComposite::build_store`'s fallback arm intentionally trips a
// `debug_assert!` for an unrecognized store name — a loud CI failure in
// debug builds (this crate's test profile, via `./scripts/test.sh` /
// nextest, has no `--release`) rather than a silent, unreviewed
// persistence decision. So the "still returns a working ephemeral store"
// contract is only observable in a RELEASE build; in the test (debug)
// profile the very same call panics on the assert. This test asserts
// THAT panic — confirming the fallback path is reached (not, e.g., an
// early error return before ever hitting `build_store`) — while the
// separate release-mode behavior (log + continue ephemeral) is exercised
// by `build_store`'s own doc-comment contract and manual verification.
#[tokio::test]
#[should_panic(expected = "unrecognized store name")]
async fn unrecognized_store_name_trips_debug_assert_in_debug_profile() {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo = build_hybrid(temp_dir.path()).await;
    let _ = repo.store_get("__unknown_future_store__").await;
}
