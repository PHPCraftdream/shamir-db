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
/// `is_durable_table_config` expects (`MetaKey::BaseIndexIndexes`'s tag).
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

// ============================================================================
// 6. `stores_list_routed`'s `TFxSet`-flattened merge (perf nit, P2 group 26):
//    `HybridRepoComposite::merge_store_names` must be BYTE-IDENTICAL to the
//    original O(names × stores) `!names.contains(&disk_name)` linear-scan
//    merge it replaced, across every edge case — including duplicate disk
//    names, which are impractical to coax out of a real
//    `FjallRepo::stores_list()`, hence testing the extracted pure merge fn
//    directly rather than only through the full `Repo::stores_list` trait
//    call.
// ============================================================================

/// The ORIGINAL O(names × stores) merge algorithm (`repo_types.rs`, pre-fix),
/// kept here only as the oracle these tests check the new
/// `TFxSet`-flattened `merge_store_names` against.
fn old_merge_oracle(mut names: Vec<String>, disk_names: Vec<String>) -> Vec<String> {
    for disk_name in disk_names {
        if !names.contains(&disk_name) {
            names.push(disk_name);
        }
    }
    names
}

#[test]
fn merge_store_names_matches_oracle_both_empty() {
    let names: Vec<String> = Vec::new();
    let disk_names: Vec<String> = Vec::new();
    assert_eq!(
        crate::repo::repo_types::HybridRepoComposite::merge_store_names(
            names.clone(),
            disk_names.clone()
        ),
        old_merge_oracle(names, disk_names)
    );
}

#[test]
fn merge_store_names_matches_oracle_empty_mem_names() {
    let names: Vec<String> = Vec::new();
    let disk_names: Vec<String> = vec!["__info__users".into(), "__interner__".into()];
    let result = crate::repo::repo_types::HybridRepoComposite::merge_store_names(
        names.clone(),
        disk_names.clone(),
    );
    assert_eq!(result, old_merge_oracle(names, disk_names));
    assert_eq!(
        result,
        vec!["__info__users".to_string(), "__interner__".to_string()]
    );
}

#[test]
fn merge_store_names_matches_oracle_disjoint_names() {
    let names: Vec<String> = vec!["__history__users".into(), "__data__users".into()];
    let disk_names: Vec<String> = vec!["__info__users".into(), "__interner__".into()];
    assert_eq!(
        crate::repo::repo_types::HybridRepoComposite::merge_store_names(
            names.clone(),
            disk_names.clone()
        ),
        old_merge_oracle(names, disk_names)
    );
}

#[test]
fn merge_store_names_matches_oracle_disk_name_already_in_mem_names() {
    // A disk name that duplicates an existing mem name must NOT be
    // appended a second time.
    let names: Vec<String> = vec!["__info__users".into(), "__tx__".into()];
    let disk_names: Vec<String> = vec!["__info__users".into(), "__interner__".into()];
    let result = crate::repo::repo_types::HybridRepoComposite::merge_store_names(
        names.clone(),
        disk_names.clone(),
    );
    assert_eq!(result, old_merge_oracle(names, disk_names));
    assert_eq!(
        result,
        vec![
            "__info__users".to_string(),
            "__tx__".to_string(),
            "__interner__".to_string(),
        ]
    );
}

#[test]
fn merge_store_names_matches_oracle_duplicate_disk_names() {
    // `disk_names` itself contains the SAME name twice — only the first
    // occurrence must be appended; the second is a no-op, matching the
    // original growing-Vec linear-scan's behavior (the first push makes
    // `names.contains` true for the second occurrence).
    let names: Vec<String> = vec!["__changelog__".into()];
    let disk_names: Vec<String> = vec![
        "__info__users".into(),
        "__info__users".into(),
        "__interner__".into(),
    ];
    let result = crate::repo::repo_types::HybridRepoComposite::merge_store_names(
        names.clone(),
        disk_names.clone(),
    );
    assert_eq!(result, old_merge_oracle(names, disk_names));
    assert_eq!(
        result,
        vec![
            "__changelog__".to_string(),
            "__info__users".to_string(),
            "__interner__".to_string(),
        ]
    );
}

/// End-to-end sanity check through the real `Repo::stores_list` trait
/// method (not just the extracted pure fn) — proves the flattened merge is
/// actually wired into `stores_list_routed` and reachable via a live
/// `BoxRepo::Hybrid`, including a name present ONLY on the disk tier
/// (`__info__`/`__interner__`) that `stores_list` must surface even though
/// no `mem` store of that name was ever created.
#[tokio::test]
async fn stores_list_surfaces_disk_only_names_via_hybrid_repo() {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo = build_hybrid(temp_dir.path()).await;

    // Only touches the mem tier.
    let _ = repo.store_get("__data__users").await.unwrap();
    // Only touches the disk tier (mirrored) — never registered in `mem`.
    let _ = repo.store_get("__info__users").await.unwrap();

    let names = repo.stores_list().await.unwrap();
    assert!(names.contains(&"__data__users".to_string()));
    assert!(names.contains(&"__info__users".to_string()));
    // No duplicates.
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        names.len(),
        "stores_list must not duplicate names: {names:?}"
    );
}
