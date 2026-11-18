use std::fs;
use std::future::Future;
use std::path::Path;

use shamir_storage::error::DbError;
use shamir_storage::types::Repo;
use shamir_types::codecs::interned::{inner_value_to_query_value, query_value_to_inner};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;
use shamir_wal::{SegmentSet, WalEntryV2, WalOpV2};
use tempfile::TempDir;
use tokio::task::yield_now;
use tokio::time::timeout;

use crate::db_instance::replay_to::{replay_to, ReplayReport, ReplayScope, ReplayToOpCounters};
use crate::meta::recovery_marker::load_last_committed;
use crate::repo::{repo_token, BoxRepoFactory, RepoFactory, RepoInstance};
use crate::table::{table_token_for, TableConfig};

use super::replay_to_tests::{entry, write_wal, BOUND};

const REPO: &str = "data";
const TABLE: &str = "widgets";
const V_STAR: u64 = 4;
const KEPT: RecordId = RecordId([1; 16]);
const DELETED: RecordId = RecordId([2; 16]);
const AFTER_SEAL: RecordId = RecordId([3; 16]);

// Cancel-safe: no for writes; timing out fails the test and discards the fixture.
async fn bounded<F: Future>(step: &str, future: F) -> F::Output {
    timeout(BOUND, Box::pin(future))
        .await
        .unwrap_or_else(|_| panic!("{step} timed out"))
}

fn row(value: &str) -> QueryValue {
    let mut fields = new_map();
    fields.insert("value".into(), QueryValue::Str(value.into()));
    QueryValue::Map(fields)
}

fn copy_store(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    for item in fs::read_dir(source).unwrap() {
        let item = item.unwrap();
        let destination = target.join(item.file_name());
        if item.file_type().unwrap().is_dir() {
            copy_store(&item.path(), &destination);
        } else {
            fs::copy(item.path(), destination).unwrap();
        }
    }
}

// Probe the actual exclusive lock. Only retry lock contention; yielding lets
// any retiring engine tasks run on this same runtime. No fixed delay or guesses.
async fn wait_for_store_unlock(store: &Path) {
    let factory = BoxRepoFactory::fjall_raw(store);
    bounded("exclusive Fjall lock probe", async {
        loop {
            match Box::pin(factory.create()).await {
                Ok(backing) => {
                    drop(backing);
                    break;
                }
                Err(DbError::Storage(message)) if message.contains("Locked") => yield_now().await,
                Err(error) => panic!("Fjall lock probe failed: {error}"),
            }
        }
    })
    .await;
}

async fn assert_ordinary_boot(store: &Path, configs: Vec<TableConfig>, post_seal: bool) {
    let repo = bounded(
        "ordinary boot",
        RepoInstance::from_factory(REPO.into(), BoxRepoFactory::fjall_raw(store), configs),
    )
    .await
    .unwrap();
    let expected_version = if post_seal { V_STAR + 1 } else { V_STAR };
    assert_eq!(
        bounded("open transaction gate", repo.tx_gate())
            .await
            .unwrap()
            .last_committed(),
        expected_version
    );
    assert_committed_history(&repo, expected_version, post_seal).await;
    bounded("stop boot journal writer", repo.seed_stop_journal_writer())
        .await
        .unwrap();
}

async fn assert_committed_history(repo: &RepoInstance, expected_version: u64, post_seal: bool) {
    let info = bounded("open tx info", repo.tx_info_store()).await.unwrap();
    assert_eq!(
        bounded("read committed marker", load_last_committed(&info))
            .await
            .unwrap(),
        Some(expected_version)
    );
    let table = bounded("open widgets", repo.get_table(TABLE))
        .await
        .unwrap();
    let manager = bounded("open interner", repo.repo_interner())
        .await
        .unwrap();
    let interner = bounded("load interner", manager.get()).await.unwrap();
    let kept = bounded("read kept row", table.get(KEPT)).await.unwrap();
    assert_eq!(
        inner_value_to_query_value(&kept, interner).unwrap(),
        row("sealed")
    );
    let deleted = bounded("read deleted row", table.get(DELETED)).await;
    assert!(matches!(deleted, Err(DbError::NotFound(_))), "{deleted:?}");
    let later = bounded("read post-seal row", table.get(AFTER_SEAL)).await;
    if post_seal {
        assert_eq!(
            inner_value_to_query_value(&later.unwrap(), interner).unwrap(),
            row("post-seal")
        );
    } else {
        assert!(matches!(later, Err(DbError::NotFound(_))), "{later:?}");
    }
}

#[tokio::test]
async fn replay_to_v_star_double_boot() {
    let source_dir = TempDir::new().unwrap();
    let staged_dir = TempDir::new().unwrap();
    let source = source_dir.path().join(REPO);
    let store = staged_dir.path().join(REPO);
    let source_wal = source.with_extension("shamirwal");
    let wal = store.with_extension("shamirwal");
    fs::create_dir(&source).unwrap();

    let physical_id = RecordId([9; 16]);
    let config = TableConfig::new(TABLE).with_store_id(physical_id);
    let factory = BoxRepoFactory::fjall_raw(&source);
    let backing = bounded("initialize Fjall", factory.create()).await.unwrap();
    // Initialize a persistent store without constructing a file WAL. The
    // crash image consists of this baseline plus the manually staged WAL.
    let repo = RepoInstance::new(REPO.into(), backing, vec![config]);
    drop(
        bounded("initialize widgets", repo.get_table(TABLE))
            .await
            .unwrap(),
    );
    let catalog_config = repo.table_config(TABLE).unwrap();
    assert_eq!(catalog_config.store_id, Some(physical_id));
    let configs = vec![catalog_config];
    let entries = {
        let manager = bounded("open fixture interner", repo.repo_interner())
            .await
            .unwrap();
        let interner = bounded("load fixture interner", manager.get())
            .await
            .unwrap();
        let token = table_token_for(TABLE);
        let put = |rid, value| WalOpV2::Put {
            table_id_interned: token,
            rid,
            body: query_value_to_inner(&row(value), interner)
                .unwrap()
                .to_bytes()
                .unwrap(),
        };
        let mut entries = [
            entry(101, 1, put(KEPT, "superseded")),
            entry(102, 2, put(DELETED, "doomed")),
            entry(103, 3, put(KEPT, "sealed")),
            entry(
                104,
                V_STAR,
                WalOpV2::Delete {
                    table_id_interned: token,
                    rid: DELETED,
                },
            ),
            entry(105, V_STAR + 1, put(AFTER_SEAL, "post-seal")),
        ];
        for entry in &mut entries {
            entry.repo_id_interned = repo_token(REPO);
        }
        bounded("persist fixture interner", manager.persist())
            .await
            .unwrap();
        entries
    };
    let backing = repo.seed_backing_repo();
    bounded("persist baseline", backing.persist_once())
        .await
        .unwrap();
    drop(backing);
    drop(repo);
    wait_for_store_unlock(&source).await;
    copy_store(&source, &store);

    // Only the source receives V*+1. Boot it to prove that the omitted
    // record is valid and would be visible if the staged WAL included it.
    bounded("stage source WAL", write_wal(&source_wal, &entries)).await;
    let sealed_entries: &[WalEntryV2] = &entries[..4];
    bounded("stage bounded WAL", write_wal(&wal, sealed_entries)).await;
    assert_ordinary_boot(&source, configs.clone(), true).await;
    wait_for_store_unlock(&source).await;

    let expected = ReplayReport {
        entries_staged: 4,
        entries_applied: 4,
        highest_version_applied: V_STAR,
        committed_version: V_STAR,
        counters: ReplayToOpCounters {
            put: 3,
            delete: 1,
            ..Default::default()
        },
    };
    for _ in 0..2 {
        wait_for_store_unlock(&store).await;
        let report = bounded(
            "strict cold replay",
            replay_to(&store, &wal, V_STAR, ReplayScope::new(configs.clone())),
        )
        .await
        .unwrap();
        assert_eq!(report, expected);
        wait_for_store_unlock(&store).await;
        // Reopen without the file WAL or boot recovery: neither can repair
        // a missing marker or missing rows before these assertions.
        let factory = BoxRepoFactory::fjall_raw(&store);
        let backing = bounded("open replayed Fjall", factory.create())
            .await
            .unwrap();
        let repo = RepoInstance::new(REPO.into(), backing, configs.clone());
        assert_committed_history(&repo, V_STAR, false).await;
        drop(repo);

        wait_for_store_unlock(&store).await;
        assert_ordinary_boot(&store, configs.clone(), false).await;
    }
    wait_for_store_unlock(&store).await;
    let retained = bounded("read retained WAL", SegmentSet::read_entries_strict(&wal))
        .await
        .unwrap();
    assert_eq!(retained.as_slice(), sealed_entries);
}
