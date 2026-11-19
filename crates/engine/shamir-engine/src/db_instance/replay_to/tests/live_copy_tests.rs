//! Strict replay of a file copy sealed while a live writer spans the
//! backup. The post-seal writer attempt is PROVEN parked while the hold
//! keeps the gate closed: the writer constructs the real commit future
//! under the hold, polls it once (deterministically `Pending` — nothing
//! has been appended), and only then does the main task stage the sealed
//! WAL. The staged WAL provably contains nothing above V* and the replay
//! converges exactly at the seal point; the hold is released before the
//! writer's commit is awaited to completion.

use std::fs;
use std::future::Future;
use std::path::Path;

use shamir_query_builder::batch::{Batch, Durability};
use shamir_query_builder::{write, Query};
use shamir_query_types::batch::VisibilityWait;
use shamir_storage::error::DbResult;
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio::time::timeout;

use super::replay_to_tests::BOUND;
use crate::db_instance::replay_to::{replay_to, ReplayScope};
use crate::query::batch::{execute_batch_unchecked, BatchResolver, TableResolver};
use crate::query::TableRef;
use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::{TableConfig, TableManager};

const REPO: &str = "data";
const TABLE: &str = "items";

struct Resolver(RepoInstance);

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table: &TableRef) -> DbResult<TableManager> {
        self.0.get_table(&table.table).await
    }
}

#[async_trait::async_trait]
impl BatchResolver for Resolver {
    async fn resolve_repo(&self, _: &str) -> DbResult<crate::tx::RepoCore> {
        Ok(self.0.core.clone())
    }
}

// A timeout fails the test and discards the fixture; writes are not retried.
async fn bounded<F: Future>(step: &str, future: F) -> F::Output {
    timeout(BOUND, Box::pin(future))
        .await
        .unwrap_or_else(|_| panic!("{step} timed out"))
}

async fn open(store: &Path, config: TableConfig) -> RepoInstance {
    let repo = bounded(
        "open Fjall repo",
        RepoInstance::from_factory(REPO.into(), BoxRepoFactory::fjall_raw(store), vec![config]),
    )
    .await
    .unwrap();
    drop(
        bounded("open configured table", repo.get_table(TABLE))
            .await
            .unwrap(),
    );
    repo
}

async fn commit(resolver: &Resolver, name: &str, visibility: VisibilityWait) -> u64 {
    let mut fields = new_map();
    fields.insert("name".into(), QueryValue::Str(name.into()));
    let mut batch = Batch::new();
    batch.transactional().visibility_wait(visibility);
    if matches!(visibility, VisibilityWait::Strict) {
        batch.durability(Durability::Synced);
    }
    batch.insert(name, write::insert(TABLE).row(QueryValue::Map(fields)));
    let response = bounded(
        name,
        execute_batch_unchecked(&batch.build(), resolver, None, None, Actor::System, "test"),
    )
    .await
    .unwrap();
    response.transaction.unwrap().commit_version.unwrap()
}

async fn rows(repo: &RepoInstance) -> Vec<String> {
    let resolver = Resolver(repo.clone());
    let mut batch = Batch::new();
    batch.query("probe", Query::from(TABLE));
    let response = bounded(
        "read probe rows",
        execute_batch_unchecked(&batch.build(), &resolver, None, None, Actor::System, "test"),
    )
    .await
    .unwrap();
    let mut names: Vec<_> = response.results["probe"]
        .records
        .iter()
        .map(|row| row.get_value_str("name").unwrap().to_owned())
        .collect();
    names.sort_unstable();
    names
}

fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        // Windows can refuse reads of the live Fjall lock file.
        if name == "lock" {
            continue;
        }
        let destination = target.join(name);
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn replay_of_live_copy_matches_source_at_seal() {
    let source_dir = TempDir::new().unwrap();
    let staged_dir = TempDir::new().unwrap();
    let source = source_dir.path().join(REPO);
    let staged = staged_dir.path().join(REPO);
    let source_wal = source.with_file_name("data.shamirwal");
    let staged_wal = staged.with_file_name("data.shamirwal");
    fs::create_dir(&source).unwrap();
    let physical_id = RecordId([9; 16]);
    let repo = open(&source, TableConfig::new(TABLE).with_store_id(physical_id)).await;
    let config = repo.table_config(TABLE).unwrap();
    assert_eq!(config.store_id, Some(physical_id));
    let pin = bounded("pin WAL history", repo.acquire_truncation_pin(0))
        .await
        .unwrap();

    let (first_tx, first_rx) = oneshot::channel();
    let (store_copied_tx, store_copied_rx) = oneshot::channel();
    let (second_tx, second_rx) = oneshot::channel();
    let (seal_ready_tx, seal_ready_rx) = oneshot::channel();
    let (attempted_tx, attempted_rx) = oneshot::channel();
    let resolver = Resolver(repo.clone());
    let mut writer = JoinSet::new();
    writer.spawn(async move {
        first_tx
            .send(commit(&resolver, "before_capture", VisibilityWait::Strict).await)
            .unwrap();
        bounded("wait for store copy", store_copied_rx)
            .await
            .unwrap();
        second_tx
            .send(commit(&resolver, "before_seal", VisibilityWait::Strict).await)
            .unwrap();
        bounded("wait for seal readiness", seal_ready_rx)
            .await
            .unwrap();
        // Under the live hold the strict commit parks on the closed
        // admission gate: one poll is deterministic `Pending` and the
        // future has appended nothing. The handshake tells the main task
        // the attempt has been initiated, so the WAL stage that follows
        // is provably pre-attempt.
        let mut pending_commit = Box::pin(commit(&resolver, "after_seal", VisibilityWait::Strict));
        assert!(
            futures::poll!(pending_commit.as_mut()).is_pending(),
            "the post-seal commit must stay pending while the seal hold \
             keeps the admission gate closed"
        );
        attempted_tx.send(()).unwrap();
        // The same pinned future is awaited to completion only after the
        // main task drops the hold.
        pending_commit.await
    });

    let first = bounded("first commit", first_rx).await.unwrap();
    let captured = bounded("capture point", repo.capture_point())
        .await
        .unwrap();
    assert_eq!(captured, first);

    // Stage the Fjall store at capture, before the writer can commit again.
    // The staged store therefore needs WAL replay to reach the seal point.
    let copied_store = staged.clone();
    bounded(
        "copy captured Fjall store",
        tokio::task::spawn_blocking(move || {
            copy_tree(&source, &copied_store)?;
            // Fjall recovery opens an existing lock file; never copy the live one.
            fs::File::create_new(copied_store.join("lock"))?;
            Ok::<(), std::io::Error>(())
        }),
    )
    .await
    .unwrap()
    .unwrap();
    store_copied_tx.send(()).unwrap();
    let second = bounded("second commit", second_rx).await.unwrap();
    assert_eq!(second, captured + 1);

    let hold = bounded("seal point", repo.seal_point()).await.unwrap();
    let v_star = hold.position();
    assert_eq!(v_star, second);
    let sealed_rows = rows(&repo).await;
    assert_eq!(sealed_rows, ["before_capture", "before_seal"]);

    // The writer's post-seal commit is initiated under the hold before the
    // sealed WAL is staged: signal seal readiness, then wait for the
    // attempt handshake before copying.
    seal_ready_tx.send(()).unwrap();
    bounded("await writer attempt under hold", attempted_rx)
        .await
        .unwrap();

    let copied_wal = staged_wal.clone();
    bounded(
        "copy sealed WAL",
        tokio::task::spawn_blocking(move || copy_tree(&source_wal, &copied_wal)),
    )
    .await
    .unwrap()
    .unwrap();
    // While the hold is STILL live the writer attempt is parked and has
    // appended nothing. Prove the horizon twice: the engine-level
    // `recoverable_through` certificate and the actual upper version of
    // the WAL frames read back from disk.
    let under_hold = bounded("source WAL horizon under hold", repo.wal_group())
        .await
        .unwrap();
    assert_eq!(
        under_hold.recoverable_through(),
        v_star,
        "the source WAL stays through V* while the hold is live"
    );
    let wal_upper = under_hold
        .replay()
        .await
        .unwrap()
        .iter()
        .map(|entry| entry.commit_version)
        .max()
        .expect("the sealed WAL must retain at least one frame");
    assert_eq!(
        wal_upper, v_star,
        "the actual WAL frames end exactly at V* while the hold is live"
    );
    drop(under_hold);

    // Release the hold BEFORE the writer's commit is awaited to
    // completion: the staged WAL keeps the sealed prefix (through V*),
    // while the source appends V*+1 only after the gate reopened.
    drop(hold);
    let third = bounded("join released writer", writer.join_next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(third, v_star + 1);
    let wal_group = bounded("read source WAL horizon", repo.wal_group())
        .await
        .unwrap();
    assert!(wal_group.recoverable_through() >= third);
    drop(wal_group);
    let gate = bounded("read source watermarks", repo.tx_gate())
        .await
        .unwrap();
    assert_eq!(gate.last_committed(), third);
    // The released writer's strict commit flushed through drain_all, so the
    // durable watermark has advanced past V*; it can never lag below the
    // seal point.
    assert!(gate.durable_watermark() >= v_star);
    drop(gate);
    drop(pin);
    assert_eq!(
        bounded("settle post-seal commit", repo.capture_point())
            .await
            .unwrap(),
        third
    );
    assert_eq!(
        rows(&repo).await,
        ["after_seal", "before_capture", "before_seal"]
    );
    bounded("shut down source", repo.shutdown_clean(BOUND))
        .await
        .unwrap();
    bounded(
        "stop source journal writer",
        repo.seed_stop_journal_writer(),
    )
    .await
    .unwrap();
    drop(repo);

    let report = bounded(
        "replay sealed copy",
        replay_to(
            &staged,
            &staged_wal,
            v_star,
            ReplayScope::new(vec![config.clone()]),
        ),
    )
    .await
    .unwrap();
    assert_eq!(report.entries_staged, 2);
    assert_eq!(report.entries_applied, 2);
    assert_eq!(report.committed_version, v_star);
    assert_eq!(report.highest_version_applied, v_star);

    let restored = open(&staged, config).await;
    let gate = bounded("read restored version", restored.tx_gate())
        .await
        .unwrap();
    assert_eq!(gate.last_committed(), v_star);
    drop(gate);
    assert_eq!(rows(&restored).await, sealed_rows);
    bounded("shut down restored repo", restored.shutdown_clean(BOUND))
        .await
        .unwrap();
    bounded(
        "stop restored journal writer",
        restored.seed_stop_journal_writer(),
    )
    .await
    .unwrap();
    drop(restored);
}
