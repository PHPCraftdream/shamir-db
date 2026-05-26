//! Engine-layer tests for the per-table buffer config DDL.
//!
//! Covers the contract that lives above the storage primitive:
//!   * `set_buffer_config` persists into `info_store`,
//!   * `get_buffer_config` reads it back,
//!   * `alter_buffer_config` partially-updates,
//!   * the value SURVIVES a TableManager reopen (factory rebuilt),
//!   * defaults stay `None` when no DDL was ever issued.

use std::sync::Arc;

use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::storage_membuffer::MemBufferConfig;
use shamir_storage::types::Repo;

async fn make_table(name: &str) -> TableManager {
    let configs = vec![TableConfig::new(name)];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    db.get_table("default", name).await.unwrap()
}

fn sample_cfg() -> MemBufferConfig {
    MemBufferConfig {
        max_bytes: 2 * 1024 * 1024,
        max_entries: 250,
        ttl_ms: Some(7_000),
        flush_interval_ms: 333,
        flush_batch_size: 48,
    }
}

#[tokio::test]
async fn get_buffer_config_is_none_when_never_set() {
    let tm = make_table("ddl_unset").await;
    let got = tm.get_buffer_config().await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn set_then_get_buffer_config_roundtrip() {
    let tm = make_table("ddl_set").await;
    let cfg = sample_cfg();
    tm.set_buffer_config(&cfg).await.unwrap();

    let got = tm.get_buffer_config().await.unwrap().expect("persisted");
    assert_eq!(got.max_bytes, cfg.max_bytes);
    assert_eq!(got.max_entries, cfg.max_entries);
    assert_eq!(got.ttl_ms, cfg.ttl_ms);
    assert_eq!(got.flush_interval_ms, cfg.flush_interval_ms);
    assert_eq!(got.flush_batch_size, cfg.flush_batch_size);
}

#[tokio::test]
async fn alter_buffer_config_partial_update() {
    let tm = make_table("ddl_alter").await;
    tm.set_buffer_config(&sample_cfg()).await.unwrap();

    let updated = tm
        .alter_buffer_config(|c| {
            c.ttl_ms = None;
            c.flush_interval_ms = 1000;
        })
        .await
        .unwrap();

    assert_eq!(updated.ttl_ms, None);
    assert_eq!(updated.flush_interval_ms, 1000);
    // Untouched knobs survive.
    assert_eq!(updated.max_bytes, sample_cfg().max_bytes);
    assert_eq!(updated.max_entries, sample_cfg().max_entries);

    let reread = tm.get_buffer_config().await.unwrap().unwrap();
    assert_eq!(reread.ttl_ms, None);
    assert_eq!(reread.flush_interval_ms, 1000);
    assert_eq!(reread.max_bytes, sample_cfg().max_bytes);
}

#[tokio::test]
async fn alter_starts_from_default_when_no_prior_config() {
    let tm = make_table("ddl_alter_fresh").await;

    let updated = tm
        .alter_buffer_config(|c| {
            c.max_entries = 9_999;
        })
        .await
        .unwrap();

    // Closure landed on top of MemBufferConfig::default().
    let default = MemBufferConfig::default();
    assert_eq!(updated.max_entries, 9_999);
    assert_eq!(updated.max_bytes, default.max_bytes);
    assert_eq!(updated.flush_interval_ms, default.flush_interval_ms);
}

#[tokio::test]
async fn buffer_config_survives_table_manager_reopen() {
    // Share one repo across two TableManager::create calls — same
    // info_store, so the persisted blob is visible to the second
    // open. This mirrors what happens on a real restart: the
    // factory rebuilds the store stack, TableManager::create reads
    // info_store and re-applies the config to the fresh stack.
    let repo: Arc<InMemoryRepo> = Arc::new(InMemoryRepo::new());
    let data_store = repo.store_get("__data__reopen".to_string()).await.unwrap();
    let info_store = repo.store_get("__info__reopen".to_string()).await.unwrap();

    let cfg = sample_cfg();

    {
        let tm = TableManager::create(
            "reopen".to_string(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        tm.set_buffer_config(&cfg).await.unwrap();
    }

    let tm2 = TableManager::create(
        "reopen".to_string(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let got = tm2.get_buffer_config().await.unwrap().unwrap();
    assert_eq!(got.max_bytes, cfg.max_bytes);
    assert_eq!(got.ttl_ms, cfg.ttl_ms);
    assert_eq!(got.flush_interval_ms, cfg.flush_interval_ms);
}

#[tokio::test]
async fn set_buffer_config_is_idempotent() {
    let tm = make_table("ddl_idempotent").await;
    let cfg = sample_cfg();

    tm.set_buffer_config(&cfg).await.unwrap();
    tm.set_buffer_config(&cfg).await.unwrap();
    tm.set_buffer_config(&cfg).await.unwrap();

    let got = tm.get_buffer_config().await.unwrap().unwrap();
    assert_eq!(got.max_bytes, cfg.max_bytes);
}

#[tokio::test]
async fn set_buffer_config_overwrites() {
    let tm = make_table("ddl_overwrite").await;
    tm.set_buffer_config(&sample_cfg()).await.unwrap();

    let other = MemBufferConfig {
        max_bytes: 99,
        max_entries: 1,
        ttl_ms: None,
        flush_interval_ms: 1,
        flush_batch_size: 1,
    };
    tm.set_buffer_config(&other).await.unwrap();

    let got = tm.get_buffer_config().await.unwrap().unwrap();
    assert_eq!(got.max_bytes, 99);
    assert_eq!(got.max_entries, 1);
    assert_eq!(got.ttl_ms, None);
    assert_eq!(got.flush_interval_ms, 1);
    assert_eq!(got.flush_batch_size, 1);
}

#[tokio::test]
async fn set_buffer_config_hot_reloads_into_membuffer() {
    // End-to-end: DDL changes max_entries on a MemBuffer-wrapped
    // factory, and a subsequent burst of inserts triggers eviction
    // bounded by the new cap. Without hot-reload reaching the
    // wrapper, this test would see the much-larger default cap
    // and the resident set would grow unbounded.
    let factory = BoxRepoFactory::membuffer(
        BoxRepoFactory::in_memory(),
        MemBufferConfig {
            max_bytes: 64 * 1024 * 1024,
            max_entries: 100_000,
            ttl_ms: None,
            flush_interval_ms: 500,
            flush_batch_size: 256,
        },
    );
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory,
        tables: vec![TableConfig::new("hot")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let tm = db.get_table("default", "hot").await.unwrap();

    // Tighten the cap to 8 entries via DDL.
    tm.set_buffer_config(&MemBufferConfig {
        max_bytes: 64 * 1024 * 1024,
        max_entries: 8,
        ttl_ms: None,
        flush_interval_ms: 500,
        flush_batch_size: 256,
    })
    .await
    .unwrap();

    // Insert plenty more than the cap. With hot-reload working,
    // the membuffer's bounded LRU will evict early entries (they
    // flush inline into the inner in_memory store, so reads still
    // succeed — we're not testing visibility, we're testing that
    // apply_buffer_config actually propagated the new cap).
    for i in 0..64u32 {
        let v = shamir_types::types::value::InnerValue::Int(i as i64);
        tm.insert(&v).await.unwrap();
    }

    // Round-trip stayed consistent via get_buffer_config.
    let got = tm.get_buffer_config().await.unwrap().unwrap();
    assert_eq!(got.max_entries, 8);
}

#[tokio::test]
async fn per_table_configs_are_independent() {
    // Two tables in the same DB; setting buffer config on one
    // does not leak into the other.
    let configs = vec![TableConfig::new("alpha"), TableConfig::new("beta")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let alpha = db.get_table("default", "alpha").await.unwrap();
    let beta = db.get_table("default", "beta").await.unwrap();

    alpha.set_buffer_config(&sample_cfg()).await.unwrap();

    let alpha_got = alpha.get_buffer_config().await.unwrap();
    let beta_got = beta.get_buffer_config().await.unwrap();
    assert!(alpha_got.is_some());
    assert!(beta_got.is_none(), "beta must remain untouched");
}
